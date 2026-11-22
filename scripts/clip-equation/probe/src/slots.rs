//! Slots assigned by instant (plan section O, C2) and the reader that picks
//! the frame for each recording instant, for M6 and M7.
//!
//! The handler computes the instant k of each frame from its composition time
//! (s_k = s_0 + k * 50 ms, s_0 = the first delivered frame) and copies the
//! frame on the GPU into the candidate slot of that instant. A frame for a
//! later instant pins the candidate. The reader, at each instant, takes the
//! last frame composed at or before s_k, reads it back only when its identity
//! (composition time) is new, and frees the older pinned slots.
//!
//! Threading rule under test (C3): multithread protection is on for the
//! capture device, the handler only issues GPU copies and never maps, and the
//! reader maps on its own thread.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
};
use windows::core::Interface;
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::clock::{self, HNS_PER_MS};
use crate::gpu::{self, MapWait, ReadBack, SendCom};
use crate::observer::{FrameTimes, HandlerClock, composition_time};
use crate::scratch::ScratchFolder;

/// 1 / f with f = 20 recorded frames per second.
pub const INSTANT_HNS: i64 = 50 * HNS_PER_MS;
/// Slots in the pool; large on purpose, so the probe sees how many are needed.
pub const SLOT_POOL: usize = 16;
/// How long after s_k the reader looks for its frame, so a frame composed just
/// before s_k has been delivered. M6 reports the delivery latency against it.
pub const READ_MARGIN_HNS: i64 = 10 * HNS_PER_MS;

/// The first instant k with s_k = `origin` + k * 50 ms at or after `time`.
pub const fn instant_at_or_after(origin: i64, time: i64) -> i64 {
    let since = time - origin;
    since.div_euclid(INSTANT_HNS) + (since.rem_euclid(INSTANT_HNS) != 0) as i64
}

struct Held {
    slot: usize,
    instant: i64,
    composed: i64,
}

/// Which slot holds the frame for which instant.
pub struct SlotTable {
    origin: Option<i64>,
    textures: Vec<ID3D11Texture2D>,
    free: Vec<usize>,
    candidate: Option<Held>,
    pinned: VecDeque<Held>,
    overflows: u64,
    max_in_use: usize,
}

impl SlotTable {
    const fn new() -> Self {
        Self {
            origin: None,
            textures: Vec::new(),
            free: Vec::new(),
            candidate: None,
            pinned: VecDeque::new(),
            overflows: 0,
            max_in_use: 0,
        }
    }

    pub const fn origin(&self) -> Option<i64> {
        self.origin
    }

    /// Frames lost because no slot was free.
    pub const fn overflows(&self) -> u64 {
        self.overflows
    }

    /// Most slots held at once (candidate plus pinned).
    pub const fn max_in_use(&self) -> usize {
        self.max_in_use
    }

    /// The reader begins after a warm-up: frees what piled up before it
    /// started (all pinned frames but the newest, which may still be the frame
    /// for the open instant) and starts the loss and slot counts afresh.
    fn begin_reading(&mut self) {
        while self.pinned.len() > 1 {
            if let Some(older) = self.pinned.pop_front() {
                self.free.push(older.slot);
            }
        }
        self.overflows = 0;
        self.max_in_use = 0;
    }

    /// The first instant k with s_k at or after `time`.
    pub fn instant_at_or_after(&self, time: i64) -> i64 {
        instant_at_or_after(self.origin.unwrap_or(time), time)
    }

    /// Handler side: the slot the frame composed at `composed` is copied into,
    /// or None when every slot is taken (a lost frame).
    fn slot_for_frame(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        composed: i64,
    ) -> Result<Option<ID3D11Texture2D>> {
        if self.textures.is_empty() {
            self.textures = (0..SLOT_POOL)
                .map(|_| gpu::frame_texture(device, width, height))
                .collect::<Result<_>>()?;
            self.free = (0..SLOT_POOL).rev().collect();
        }
        self.origin.get_or_insert(composed);
        let instant = self.instant_at_or_after(composed);
        if let Some(candidate) = &mut self.candidate
            && candidate.instant == instant
        {
            candidate.composed = composed;
            return Ok(Some(self.textures[candidate.slot].clone()));
        }
        if let Some(older) = self.candidate.take() {
            self.pinned.push_back(older);
        }
        let Some(slot) = self.free.pop() else {
            self.overflows += 1;
            return Ok(None);
        };
        self.candidate = Some(Held {
            slot,
            instant,
            composed,
        });
        self.max_in_use = self.max_in_use.max(SLOT_POOL - self.free.len());
        Ok(Some(self.textures[slot].clone()))
    }

    /// Reader side: the last frame composed at or before s_k, with its
    /// composition time. Pins the candidate when it is that frame and frees
    /// the pinned slots before it.
    fn frame_for_instant(&mut self, k: i64) -> Option<(ID3D11Texture2D, i64)> {
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.instant <= k)
            && let Some(candidate) = self.candidate.take()
        {
            self.pinned.push_back(candidate);
        }
        let position = self.pinned.iter().rposition(|held| held.instant <= k)?;
        for _ in 0..position {
            if let Some(older) = self.pinned.pop_front() {
                self.free.push(older.slot);
            }
        }
        self.pinned
            .front()
            .map(|held| (self.textures[held.slot].clone(), held.composed))
    }
}

/// What the handler and the reader share.
pub struct SlotShared {
    table: Mutex<SlotTable>,
    gpu: OnceLock<SendCom<(ID3D11Device, ID3D11DeviceContext)>>,
}

impl SlotShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(SlotTable::new()),
            gpu: OnceLock::new(),
        })
    }

    pub fn table(&self) -> MutexGuard<'_, SlotTable> {
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct InstantSlotsFlags {
    pub shared: Arc<SlotShared>,
}

/// The capture handler: GPU copy into the instant's slot, never a Map.
pub struct InstantSlots {
    shared: Arc<SlotShared>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    frames: Vec<FrameTimes>,
}

impl InstantSlots {
    pub fn frames(&self) -> &[FrameTimes] {
        &self.frames
    }
}

impl GraphicsCaptureApiHandler for InstantSlots {
    type Flags = InstantSlotsFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        let protection: ID3D11Multithread = ctx
            .device_context
            .cast()
            .map_err(|error| format!("ID3D11Multithread: {error}"))?;
        // SAFETY: switches on the device's own lock; returns the previous state.
        let _ = unsafe { protection.SetMultithreadProtected(true) };
        let _ = ctx
            .flags
            .shared
            .gpu
            .set(SendCom((ctx.device.clone(), ctx.device_context.clone())));
        Ok(Self {
            shared: ctx.flags.shared,
            device: ctx.device,
            context: ctx.device_context,
            frames: Vec::with_capacity(4096),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        let clock = HandlerClock::start();
        let composed = composition_time(frame);
        {
            // The table stays locked while the copy is issued, so the reader
            // never pins a slot whose copy is not yet queued.
            let mut table = self.shared.table();
            if let Some(slot) =
                table.slot_for_frame(&self.device, frame.width(), frame.height(), composed)?
            {
                // SAFETY: same size and format; the device is multithread protected.
                unsafe { self.context.CopyResource(&slot, frame.as_raw_texture()) };
            }
        }
        self.frames.push(clock.finish(composed));
        Ok(())
    }
}

/// One instant as the reader handled it.
#[derive(Clone, Copy, Debug)]
pub struct InstantRecord {
    pub done: i64,
    /// Instants already closed but not yet handled when this one was done.
    pub lateness: i64,
    pub new_frame: bool,
    pub read_ms: f64,
    pub map_retries: u32,
    pub read_cycles: u64,
}

/// Picks the frame for each instant until `until_hns`, reads back new frames
/// with `wait`, and writes them raw to `spool` when one is given.
pub fn run_reader(
    shared: &SlotShared,
    wait: MapWait,
    until_hns: i64,
    mut spool: Option<&mut ScratchFolder>,
) -> Result<Vec<InstantRecord>> {
    let (device, context) = wait_for(until_hns, || {
        shared
            .gpu
            .get()
            .map(|gpu| (gpu.0.0.clone(), gpu.0.1.clone()))
    })?;
    let origin = wait_for(until_hns, || shared.table().origin())?;
    let mut readback: Option<ReadBack> = None;
    let mut last_identity = None;
    let mut records = Vec::new();
    // Start at the instant still open now; older pinned frames are freed by the first pick.
    let first = {
        let mut table = shared.table();
        table.begin_reading();
        table.instant_at_or_after(clock::now_hns())
    };
    for k in first.. {
        let due = origin + k * INSTANT_HNS + READ_MARGIN_HNS;
        if due >= until_hns {
            break;
        }
        clock::sleep_until(due);
        let (start, start_cycles) = (clock::now_hns(), clock::thread_cycles());
        let chosen = shared.table().frame_for_instant(k);
        let mut record = InstantRecord {
            done: 0,
            lateness: 0,
            new_frame: false,
            read_ms: 0.0,
            map_retries: 0,
            read_cycles: 0,
        };
        if let Some((texture, composed)) = chosen
            && last_identity != Some(composed)
        {
            last_identity = Some(composed);
            record.new_frame = true;
            let (width, height) = texture_size(&texture);
            if readback
                .as_ref()
                .is_none_or(|current| current.size() != (width, height))
            {
                readback = Some(ReadBack::new(&device, width, height)?);
            }
            let Some(current) = readback.as_mut() else {
                return Err("no read-back buffer".to_owned());
            };
            record.map_retries = current.read(&context, &texture, wait)?;
            if let Some(folder) = spool.as_deref_mut() {
                folder.write_frame(current.pixels())?;
            }
        }
        record.done = clock::now_hns();
        record.read_ms = clock::hns_to_ms(record.done - start);
        record.read_cycles = clock::thread_cycles() - start_cycles;
        record.lateness = shared.table().instant_at_or_after(record.done) - k - 1;
        records.push(record);
    }
    Ok(records)
}

fn wait_for<T>(until_hns: i64, mut probe: impl FnMut() -> Option<T>) -> Result<T> {
    loop {
        if let Some(value) = probe() {
            return Ok(value);
        }
        if clock::now_hns() >= until_hns {
            return Err("the capture delivered no frame in time".to_owned());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn texture_size(texture: &ID3D11Texture2D) -> (u32, u32) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: one out-parameter.
    unsafe { texture.GetDesc(&raw mut desc) };
    (desc.Width, desc.Height)
}
