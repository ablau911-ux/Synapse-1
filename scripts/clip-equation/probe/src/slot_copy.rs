//! The observer's handler for M2 and M3: per delivered frame, one GPU copy
//! into a slot texture plus the composition time. No read-back.
//!
//! For M3 it can also read the probe window's paint counter: a 1-pixel-high
//! row through the counter blocks is copied into a small staging texture and
//! mapped eight frames later with "do not wait", so the handler never waits
//! for the GPU.

use std::collections::VecDeque;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::gpu::{self, MapWait};
use crate::observer::{FrameTimes, HandlerClock, composition_time};
use crate::probe_window::{COUNTER_BITS, COUNTER_BLOCK_PX};

/// Slot textures the copies rotate through (M2 measures the copy, not the slot rule).
const SLOT_RING: usize = 4;

pub struct SlotCopyFlags {
    pub read_counter: bool,
}

pub struct SlotCopy {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    slots: Vec<ID3D11Texture2D>,
    slot_size: (u32, u32),
    next_slot: usize,
    counter: Option<CounterProbe>,
    frames: Vec<FrameTimes>,
}

impl SlotCopy {
    pub fn frames(&self) -> &[FrameTimes] {
        &self.frames
    }

    /// The paint counter seen in each frame (12 bits), after the capture stopped.
    pub fn counters(&mut self) -> Result<Vec<Option<u32>>> {
        match &mut self.counter {
            Some(probe) => {
                probe.drain(&self.context)?;
                Ok(probe.values.clone())
            }
            None => Ok(Vec::new()),
        }
    }
}

impl GraphicsCaptureApiHandler for SlotCopy {
    type Flags = SlotCopyFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        let counter = if ctx.flags.read_counter {
            Some(CounterProbe::new(&ctx.device)?)
        } else {
            None
        };
        Ok(Self {
            device: ctx.device,
            context: ctx.device_context,
            slots: Vec::new(),
            slot_size: (0, 0),
            next_slot: 0,
            counter,
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
        let size = (frame.width(), frame.height());
        if size != self.slot_size {
            self.slots = (0..SLOT_RING)
                .map(|_| gpu::frame_texture(&self.device, size.0, size.1))
                .collect::<Result<_>>()?;
            self.slot_size = size;
        }
        let source = frame.as_raw_texture();
        // SAFETY: slot and frame have the same size and format.
        unsafe {
            self.context
                .CopyResource(&self.slots[self.next_slot], source)
        };
        self.next_slot = (self.next_slot + 1) % SLOT_RING;
        if let Some(probe) = &mut self.counter {
            probe.copy(&self.context, source)?;
        }
        self.frames.push(clock.finish(composed));
        Ok(())
    }
}

const PROBE_RING: usize = 16;
const PROBE_LAG: usize = 8;
const PROBE_ROW_Y: u32 = COUNTER_BLOCK_PX / 2;
const PROBE_WIDTH: u32 = COUNTER_BITS * COUNTER_BLOCK_PX;

/// Reads the paint counter out of captured frames without stalling the handler.
struct CounterProbe {
    ring: Vec<ID3D11Texture2D>,
    /// Frame index and ring slot of copies not yet read.
    pending: VecDeque<(usize, usize)>,
    values: Vec<Option<u32>>,
}

impl CounterProbe {
    fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            ring: (0..PROBE_RING)
                .map(|_| gpu::staging_texture(device, PROBE_WIDTH, 1))
                .collect::<Result<_>>()?,
            pending: VecDeque::new(),
            values: Vec::new(),
        })
    }

    fn copy(&mut self, context: &ID3D11DeviceContext, frame: &ID3D11Texture2D) -> Result<()> {
        let index = self.values.len();
        self.values.push(None);
        let slot = index % PROBE_RING;
        let row = D3D11_BOX {
            left: 0,
            top: PROBE_ROW_Y,
            front: 0,
            right: PROBE_WIDTH,
            bottom: PROBE_ROW_Y + 1,
            back: 1,
        };
        // SAFETY: the box lies inside the frame; the ring texture is PROBE_WIDTH x 1.
        unsafe {
            context.CopySubresourceRegion(
                &self.ring[slot],
                0,
                0,
                0,
                0,
                frame,
                0,
                Some(&raw const row),
            )
        };
        self.pending.push_back((index, slot));
        while self.pending.len() > PROBE_LAG {
            // The oldest copy must be read before its ring slot comes round again.
            let wait = if self.pending.len() >= PROBE_RING - 1 {
                MapWait::Block
            } else {
                MapWait::PollYield
            };
            if !self.read_oldest(context, wait, true)? {
                break;
            }
        }
        Ok(())
    }

    /// Reads every copy still pending (after the capture stopped).
    fn drain(&mut self, context: &ID3D11DeviceContext) -> Result<()> {
        while self.read_oldest(context, MapWait::Block, false)? {}
        Ok(())
    }

    /// Reads the oldest pending copy. With `give_up_when_busy`, a GPU that is
    /// still drawing leaves it pending and returns false.
    fn read_oldest(
        &mut self,
        context: &ID3D11DeviceContext,
        wait: MapWait,
        give_up_when_busy: bool,
    ) -> Result<bool> {
        let Some(&(index, slot)) = self.pending.front() else {
            return Ok(false);
        };
        let staging = &self.ring[slot];
        let mapped = if give_up_when_busy && wait == MapWait::PollYield {
            match gpu::try_map_for_read(context, staging)? {
                Some(mapped) => mapped,
                None => return Ok(false),
            }
        } else {
            gpu::map_for_read(context, staging, MapWait::Block)?.0
        };
        let mut value = 0;
        for bit in 0..COUNTER_BITS {
            let x = (bit * COUNTER_BLOCK_PX + COUNTER_BLOCK_PX / 2) as usize;
            // SAFETY: the mapped row holds PROBE_WIDTH BGRA pixels; byte 1 is green.
            let green = unsafe { *mapped.pData.cast::<u8>().add(x * 4 + 1) };
            if green > 127 {
                value |= 1 << bit;
            }
        }
        // SAFETY: mapped above.
        unsafe { context.Unmap(staging, 0) };
        self.values[index] = Some(value);
        self.pending.pop_front();
        Ok(true)
    }
}
