//! The handler for M4: every delivered frame is read back and hashed, so the
//! analysis can tell a new frame by identity (a new composition time, the
//! product's rule in plan A7) from a new frame by content (different pixels).

use std::hash::{DefaultHasher, Hash, Hasher};

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::gpu::{MapWait, ReadBack};
use crate::observer::{FrameTimes, HandlerClock, composition_time};

pub struct HashFrames {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    readback: Option<ReadBack>,
    frames: Vec<FrameTimes>,
    hashes: Vec<u64>,
}

impl HashFrames {
    pub fn frames(&self) -> &[FrameTimes] {
        &self.frames
    }

    pub fn hashes(&self) -> &[u64] {
        &self.hashes
    }
}

impl GraphicsCaptureApiHandler for HashFrames {
    type Flags = ();
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self {
            device: ctx.device,
            context: ctx.device_context,
            readback: None,
            frames: Vec::with_capacity(4096),
            hashes: Vec::with_capacity(4096),
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
        if self
            .readback
            .as_ref()
            .is_none_or(|readback| readback.size() != size)
        {
            self.readback = Some(ReadBack::new(&self.device, size.0, size.1)?);
        }
        let Some(readback) = self.readback.as_mut() else {
            return Err("no read-back buffer".to_owned());
        };
        readback.read(&self.context, frame.as_raw_texture(), MapWait::Block)?;
        let mut hasher = DefaultHasher::new();
        readback.pixels().hash(&mut hasher);
        self.hashes.push(hasher.finish());
        self.frames.push(clock.finish(composed));
        Ok(())
    }
}
