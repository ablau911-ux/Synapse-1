//! H.264 clip decoder on the Media Foundation Source Reader ("Bug clip video"
//! research, section 8; building plan Part 2 "Decoding").
//!
//! `ClipDecoder` owns one Source Reader over one segment file of a clip. The
//! reader's own video processing converts the decoded stream to RGB32, so
//! every frame comes back as top-down BGRA with its sample timestamp.
//!
//! All calls must stay on the thread that created the decoder: it initialises
//! COM (MTA) and Media Foundation for that thread and shuts both down on drop.

use std::path::{Path, PathBuf};

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_FRAME_RATE,
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

const HNS_PER_SECOND: u64 = 10_000_000;
const PACKED_SHIFT: u32 = 32;
const PACKED_LOW_MASK: u64 = u32::MAX as u64;
const HALF_SAMPLE: i64 = 2;
const BGRA_BYTES_PER_PIXEL: usize = 4;
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

/// A sample-table keyframe. Its time belongs to the file, not the instant clock.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(from = "(u32, i64)", into = "(u32, i64)")]
pub(super) struct KeyFrame {
    pub(super) frame: u32,
    pub(super) t_hns: i64,
}

impl From<(u32, i64)> for KeyFrame {
    fn from((frame, t_hns): (u32, i64)) -> Self {
        Self { frame, t_hns }
    }
}

impl From<KeyFrame> for (u32, i64) {
    fn from(key: KeyFrame) -> Self {
        (key.frame, key.t_hns)
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
    last: Option<(u32, DecodedFrame)>,
    half_sample_hns: i64,
    path: PathBuf,
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
            half_sample_hns: 0,
            path: path.to_owned(),
            _runtime: runtime,
        };
        decoder.read_output_type()?;
        Ok(decoder)
    }

    /// The next frame in decode order, or `None` at the end of the stream.
    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, String> {
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

    /// D6 maps instants by sample count. File times are used only to land on a keyframe.
    pub(super) fn frame_number(&mut self, number: u32, keys: &[KeyFrame]) -> Result<DecodedFrame, String> {
        if let Some((last_number, last)) = &self.last
            && *last_number == number
        {
            return Ok(last.clone());
        }
        let key = keys.iter().rev().find(|key| key.frame <= number)
            .ok_or_else(|| format!("{} has no keyframe before frame {number}", self.path.display()))?;
        let reading_on = self.last.as_ref().filter(|(last_number, _)| {
            *last_number + 1 == number || (key.frame <= *last_number && *last_number < number)
        }).map(|(last_number, _)| *last_number + 1);
        let first = match reading_on {
            Some(next) => next,
            None => {
                self.seek(key.t_hns)?;
                key.frame
            }
        };
        for current in first..=number {
            let frame = self.next_frame()?.ok_or_else(|| {
                format!("{} has no decoded frame {current}", self.path.display())
            })?;
            if reading_on.is_none() && current == first
                && frame.t_hns.abs_diff(key.t_hns) > self.half_sample_hns as u64
            {
                self.last = None;
                return Err(format!(
                    "{} seek to keyframe {} at {} hns landed at {} hns",
                    self.path.display(), key.frame, key.t_hns, frame.t_hns,
                ));
            }
            self.last = Some((current, frame));
        }
        self.last.as_ref().map(|(_, frame)| frame.clone())
            .ok_or_else(|| format!("{} has no decoded frame {number}", self.path.display()))
    }

    fn seek(&mut self, t_hns: i64) -> Result<(), String> {
        let mut position = PROPVARIANT::default();
        // SAFETY: the union tag and value match; VT_I8 owns no resources.
        unsafe {
            let inner = &mut *position.Anonymous.Anonymous;
            inner.vt = VT_I8;
            inner.Anonymous.hVal = t_hns;
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
        self.width = u32::try_from(size >> PACKED_SHIFT).unwrap_or(0);
        self.height = u32::try_from(size & PACKED_LOW_MASK).unwrap_or(0);
        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "the decoder reported an empty frame size {}x{}",
                self.width, self.height
            ));
        }
        // The file's rate defines the tolerance for validating a keyframe seek.
        let rate = unsafe { current.GetUINT64(&MF_MT_FRAME_RATE) }
            .map_err(hr("MF_MT_FRAME_RATE"))?;
        let numerator = rate >> PACKED_SHIFT;
        let denominator = rate & PACKED_LOW_MASK;
        if numerator == 0 || denominator == 0 {
            return Err("the decoder reported an invalid frame rate".into());
        }
        self.half_sample_hns = (HNS_PER_SECOND * denominator / numerator) as i64 / HALF_SAMPLE;
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
        let row = width * BGRA_BYTES_PER_PIXEL;
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
        if data.is_null() || stride < row || stride * height > length {
            return Err(format!(
                "decoded buffer of {length} bytes is too small for {width}x{height} RGB32"
            ));
        }
        unlocked?;
        Ok(out)
    }
}

/// Compressed samples in file order, including timestamps and actual keyframe flags.
pub(super) struct SampleTable {
    samples: Vec<(i64, bool)>,
}

impl SampleTable {
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        let _runtime = MediaFoundationRuntime::start()?;
        let url = HSTRING::from(path.as_os_str());
        // SAFETY: the native output type reads compressed samples without decoding.
        let reader = unsafe { MFCreateSourceReaderFromURL(&url, None) }
            .map_err(hr("MFCreateSourceReaderFromURL"))?;
        let mut samples = Vec::new();
        let mut empty_reads = 0;
        loop {
            let mut flags = 0_u32;
            let mut timestamp = 0_i64;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: the three out-parameters live through the call.
            unsafe {
                reader.ReadSample(
                    video_stream(), 0, None, Some(&raw mut flags),
                    Some(&raw mut timestamp), Some(&raw mut sample),
                )
            }.map_err(hr("IMFSourceReader::ReadSample"))?;
            if flags & flag(MF_SOURCE_READERF_ERROR.0) != 0 {
                return Err("the source reader reported a stream error".to_owned());
            }
            if let Some(sample) = sample {
                if samples.len() == u32::MAX as usize {
                    return Err("sample table exceeds the frame-number range".into());
                }
                // SAFETY: a plain query on the compressed sample.
                let key = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }
                    .unwrap_or(0) != 0;
                samples.push((timestamp, key));
                empty_reads = 0;
            } else {
                empty_reads += 1;
            }
            if flags & flag(MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
                return Ok(Self { samples });
            }
            if empty_reads >= MAX_EMPTY_READS {
                return Err(format!("sample table returned {MAX_EMPTY_READS} empty reads in a row"));
            }
        }
    }

    pub(super) fn len(&self) -> u32 {
        self.samples.len() as u32
    }

    pub(super) fn key_frames(&self) -> Vec<KeyFrame> {
        self.samples.iter().enumerate().filter_map(|(frame, &(t_hns, key))| {
            key.then_some(KeyFrame { frame: frame as u32, t_hns })
        }).collect()
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
