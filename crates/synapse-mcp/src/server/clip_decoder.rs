//! H.264 clip decoder on the Media Foundation Source Reader ("Bug clip video"
//! research, section 8; building plan Part 2 "Decoding").
//!
//! `ClipDecoder` owns one Source Reader over one segment file of a clip. The
//! reader's own video processing converts the decoded stream to RGB32, so
//! every frame comes back as top-down BGRA with its sample timestamp.
//!
//! All calls must stay on the thread that created the decoder: it initialises
//! COM (MTA) and Media Foundation for that thread and shuts both down on drop.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFSampleExtension_CleanPoint,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows::core::{GUID, HSTRING};

use super::clip_encoder::{MediaFoundationRuntime, hr};

const HNS_PER_MS: i64 = 10_000;
/// D7: a target ahead of the last decoded frame is reached by reading on for
/// at most one GOP of decoded frames; farther targets seek to the keyframe
/// before them, so seek cost is bounded in frames, not in time.
const FORWARD_READ_LIMIT_FRAMES: u32 = super::clip_encoder::GOP_FRAMES;
/// Empty reads (stream ticks) tolerated in a row before giving up.
const MAX_EMPTY_READS: u32 = 1_000;

/// One decoded frame.
#[derive(Clone)]
pub(super) struct DecodedFrame {
    /// Sample timestamp in 100 ns units from the start of the stream.
    pub(super) t_hns: i64,
    pub(super) width: u32,
    pub(super) height: u32,
    /// Top-down BGRA (alpha undefined), `width * 4` bytes per row.
    pub(super) bgra: Vec<u8>,
}

impl DecodedFrame {
    pub(super) fn t_ms(&self) -> u64 {
        u64::try_from((self.t_hns + HNS_PER_MS / 2) / HNS_PER_MS).unwrap_or(0)
    }
}

/// The Source Reader of one segment file (`segment-NNNN.mp4`), with RGB32 output.
pub(super) struct ClipDecoder {
    // Field order matters: the reader is released before Media Foundation
    // shuts down (fields drop in declaration order).
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    /// Bytes per row reported by the output type; negative means bottom-up.
    stride: i32,
    /// The most recent frame returned, for forward reads without a seek.
    last: Option<DecodedFrame>,
    _runtime: MediaFoundationRuntime,
}

impl ClipDecoder {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let runtime = MediaFoundationRuntime::start()?;
        let reader = create_reader(path)?;
        let mut decoder = Self {
            reader,
            width: 0,
            height: 0,
            stride: 0,
            last: None,
            _runtime: runtime,
        };
        decoder.read_output_type()?;
        Ok(decoder)
    }

    /// The next frame in decode order, or `None` at the end of the stream.
    pub(super) fn next_frame(&mut self) -> Result<Option<DecodedFrame>, String> {
        let stream = video_stream();
        for _ in 0..MAX_EMPTY_READS {
            let mut flags = 0_u32;
            let mut timestamp = 0_i64;
            let mut sample: Option<IMFSample> = None;
            unsafe {
                self.reader.ReadSample(
                    stream,
                    0,
                    None,
                    Some(&raw mut flags),
                    Some(&raw mut timestamp),
                    Some(&raw mut sample),
                )
            }
            .map_err(hr("IMFSourceReader::ReadSample"))?;
            if flags & flag(MF_SOURCE_READERF_ERROR.0) != 0 {
                return Err("the source reader reported a stream error".to_owned());
            }
            if flags & flag(MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0) != 0 {
                self.read_output_type()?;
            }
            if let Some(sample) = sample {
                let frame = DecodedFrame {
                    t_hns: timestamp.max(0),
                    width: self.width,
                    height: self.height,
                    bgra: self.copy_pixels(&sample)?,
                };
                self.last = Some(frame.clone());
                return Ok(Some(frame));
            }
            if flags & flag(MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
                return Ok(None);
            }
        }
        Err(format!(
            "the source reader returned {MAX_EMPTY_READS} empty reads in a row"
        ))
    }

    /// The frame on screen at `t_ms`: the last frame whose time is at most
    /// `t_ms` plus half a frame (`frame_ms / 2`). Frames come at uneven times
    /// (repeats are not encoded). Reads on from the previous frame for at most
    /// `FORWARD_READ_LIMIT_FRAMES` frames, otherwise seeks first.
    pub(super) fn frame_at(&mut self, t_ms: u64, frame_ms: u64) -> Result<DecodedFrame, String> {
        let tolerance = frame_ms / 2;
        if let Some(last) = &self.last
            && last.t_ms() <= t_ms + tolerance
            && last.t_ms() + tolerance >= t_ms
        {
            return Ok(last.clone());
        }
        let mut reading_on = self.last.as_ref().is_some_and(|last| last.t_ms() <= t_ms);
        if !reading_on {
            self.seek(t_ms)?;
        }
        let mut previous = self.last.take();
        let mut reads = 0_u32;
        loop {
            let Some(frame) = self.next_frame()? else {
                // Past the end: the last frame stays on screen.
                return previous.ok_or_else(|| format!("no frame decodes at {t_ms} ms"));
            };
            if frame.t_ms() > t_ms + tolerance {
                return Ok(previous.unwrap_or(frame));
            }
            if frame.t_ms() + tolerance >= t_ms {
                return Ok(frame);
            }
            previous = Some(frame);
            reads += 1;
            if reading_on && reads >= FORWARD_READ_LIMIT_FRAMES {
                self.seek(t_ms)?;
                previous = None;
                reading_on = false;
            }
        }
    }

    fn seek(&mut self, t_ms: u64) -> Result<(), String> {
        let mut position = PROPVARIANT::default();
        // SAFETY: writing the tag and the matching union member of a fresh
        // PROPVARIANT; VT_I8 owns no resources.
        unsafe {
            let inner = &mut *position.Anonymous.Anonymous;
            inner.vt = VT_I8;
            inner.Anonymous.hVal = i64::try_from(t_ms)
                .unwrap_or(i64::MAX / HNS_PER_MS)
                .saturating_mul(HNS_PER_MS);
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &raw const position)
                .map_err(hr("IMFSourceReader::SetCurrentPosition"))?;
        }
        self.last = None;
        Ok(())
    }

    fn read_output_type(&mut self) -> Result<(), String> {
        let current = unsafe { self.reader.GetCurrentMediaType(video_stream()) }
            .map_err(hr("IMFSourceReader::GetCurrentMediaType"))?;
        let size =
            unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(hr("MF_MT_FRAME_SIZE"))?;
        self.width = u32::try_from(size >> 32).unwrap_or(0);
        self.height = u32::try_from(size & 0xFFFF_FFFF).unwrap_or(0);
        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "the decoder reported an empty frame size {}x{}",
                self.width, self.height
            ));
        }
        // MF stores a negative stride (bottom-up rows) as its two's complement.
        self.stride = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map_or(0, |stride| i32::from_ne_bytes(stride.to_ne_bytes()));
        Ok(())
    }

    fn copy_pixels(&self, sample: &IMFSample) -> Result<Vec<u8>, String> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(hr("IMFSample::ConvertToContiguousBuffer"))?;
        let mut data = std::ptr::null_mut::<u8>();
        let mut length = 0_u32;
        unsafe { buffer.Lock(&raw mut data, None, Some(&raw mut length)) }
            .map_err(hr("IMFMediaBuffer::Lock"))?;
        let (width, height) = (self.width as usize, self.height as usize);
        let row = width * 4;
        let length = length as usize;
        let mut stride = match self.stride.unsigned_abs() as usize {
            0 => row,
            stride => stride,
        };
        if stride * height > length && length >= row * height {
            stride = length / height;
        }
        let bottom_up = self.stride < 0;
        let mut out = vec![0_u8; row * height];
        if !data.is_null() && stride >= row && stride * height <= length {
            // SAFETY: `data` points at `length` readable bytes until Unlock.
            let bytes = unsafe { std::slice::from_raw_parts(data, length) };
            for (y, target) in out.chunks_exact_mut(row).enumerate() {
                let source_row = if bottom_up { height - 1 - y } else { y };
                let start = source_row * stride;
                target.copy_from_slice(&bytes[start..start + row]);
            }
        }
        let unlocked = unsafe { buffer.Unlock() }.map_err(hr("IMFMediaBuffer::Unlock"));
        if stride < row || stride * height > length {
            return Err(format!(
                "decoded buffer of {length} bytes is too small for {width}x{height} RGB32"
            ));
        }
        unlocked?;
        Ok(out)
    }
}

/// The times (100 ns, within the file) of the keyframes of a finished segment,
/// read from its compressed samples (B8: keyframe instants come from the
/// file, not from the settings).
pub(super) fn keyframe_times_hns(path: &Path) -> Result<Vec<i64>, String> {
    let _runtime = MediaFoundationRuntime::start()?;
    let url = HSTRING::from(path.as_os_str());
    // SAFETY: a reader over the file with its native (compressed) output type.
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, None) }
        .map_err(hr("MFCreateSourceReaderFromURL"))?;
    let mut times = Vec::new();
    loop {
        let mut flags = 0_u32;
        let mut timestamp = 0_i64;
        let mut sample: Option<IMFSample> = None;
        // SAFETY: three out-parameters.
        unsafe {
            reader.ReadSample(
                video_stream(),
                0,
                None,
                Some(&raw mut flags),
                Some(&raw mut timestamp),
                Some(&raw mut sample),
            )
        }
        .map_err(hr("IMFSourceReader::ReadSample"))?;
        if flags & flag(MF_SOURCE_READERF_ERROR.0) != 0 {
            return Err("the source reader reported a stream error".to_owned());
        }
        if let Some(sample) = sample
            // SAFETY: a plain attribute query on the sample.
            && unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0
        {
            times.push(timestamp);
        }
        if flags & flag(MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
            return Ok(times);
        }
    }
}

fn create_reader(path: &Path) -> Result<IMFSourceReader, String> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&raw mut attributes, 1) }.map_err(hr("MFCreateAttributes"))?;
    let attributes = attributes.ok_or("MFCreateAttributes returned no attribute store")?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1) }
        .map_err(hr("MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING"))?;
    let url = HSTRING::from(path.as_os_str());
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, &attributes) }
        .map_err(hr("MFCreateSourceReaderFromURL"))?;
    let output = unsafe { MFCreateMediaType() }.map_err(hr("MFCreateMediaType"))?;
    unsafe {
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(hr("MF_MT_MAJOR_TYPE"))?;
        output
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(hr("MF_MT_SUBTYPE"))?;
        reader
            .SetCurrentMediaType(video_stream(), None, &output)
            .map_err(hr("IMFSourceReader::SetCurrentMediaType(RGB32)"))?;
    }
    Ok(reader)
}

const fn video_stream() -> u32 {
    flag(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0)
}

/// Media Foundation passes these i32 constants as u32 bit patterns.
const fn flag(value: i32) -> u32 {
    u32::from_ne_bytes(value.to_ne_bytes())
}
