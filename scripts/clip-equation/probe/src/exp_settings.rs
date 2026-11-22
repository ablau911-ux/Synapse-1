//! The experiment factors of one run (docs/clip-recorder-experiments-plan.md,
//! "Factors"). One run is one probe process, so the settings are installed
//! once per process and read by the copied product code (`clip_encoder.rs`,
//! `clip_folder.rs`) where the product has a fixed constant.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::AtomicI8;

use windows::Win32::Media::MediaFoundation::{
    IMFDXGIDeviceManager, MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC,
    eAVEncH264VProfile_High, eAVEncH265VProfile_Main_420_8,
};
use windows::core::GUID;

/// The compressed format the encoder writes (F2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "h264" => Some(Self::H264),
            "hevc" => Some(Self::Hevc),
            "av1" => Some(Self::Av1),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    pub const fn subtype(self) -> GUID {
        match self {
            Self::H264 => MFVideoFormat_H264,
            Self::Hevc => MFVideoFormat_HEVC,
            Self::Av1 => MFVideoFormat_AV1,
        }
    }

    /// `MF_MT_MPEG2_PROFILE` for the output type: High for H.264 (as the
    /// product), Main 4:2:0 8-bit for HEVC; AV1 is left to the encoder.
    pub const fn profile(self) -> Option<u32> {
        match self {
            Self::H264 => Some(eAVEncH264VProfile_High.0 as u32),
            Self::Hevc => Some(eAVEncH265VProfile_Main_420_8.0 as u32),
            Self::Av1 => None,
        }
    }
}

/// One run's encoder factors. `Default` is the product's fixed values.
#[derive(Clone, Debug)]
pub struct EncodeSettings {
    pub codec: Codec,
    /// Hardware transforms on (the product's first try).
    pub hardware: bool,
    /// LUID of the adapter whose encoder is wanted; a DXGI device manager on
    /// that adapter is handed to the Sink Writer.
    pub adapter_luid: Option<i64>,
    /// q; `None` = the bitrate fallback is used directly (F3, β).
    pub quality: Option<u32>,
    /// β, bits per pixel and frame.
    pub bits_per_pixel_frame: f64,
    pub gop_frames: u32,
    pub screen_hint: bool,
    pub low_latency: bool,
    /// zstd level of the waiting frames (D: −3; Z: −3, 1, 3, 9).
    pub zstd_level: i32,
    /// D0: waiting frames are written and read raw, without zstd.
    pub raw_waiting: bool,
}

impl EncodeSettings {
    /// The factors from `key=value` arguments; anything not given keeps the
    /// product's value.
    pub fn from_args(args: &crate::exp_meter::Args) -> crate::Result<Self> {
        let product = Self::default();
        let codec = args.text_or("codec", "h264");
        let quality = match args.text_or("q", "70") {
            "none" => None,
            text => Some(text.parse().map_err(|_| format!("q={text} is not a number"))?),
        };
        let adapter_luid = match args.text_or("adapter", "none") {
            "none" => None,
            text => Some(text.parse().map_err(|_| format!("adapter={text} is not a LUID"))?),
        };
        Ok(Self {
            codec: Codec::parse(codec).ok_or_else(|| format!("unknown codec={codec}"))?,
            hardware: args.flag("hw", product.hardware),
            adapter_luid,
            quality,
            bits_per_pixel_frame: args.number("beta", product.bits_per_pixel_frame)?,
            gop_frames: args.number("gop", product.gop_frames)?,
            screen_hint: args.flag("hint", product.screen_hint),
            low_latency: args.flag("ll", product.low_latency),
            zstd_level: args.number("level", product.zstd_level)?,
            raw_waiting: args.text_or("waiting_format", "zstd") == "raw",
        })
    }
}

impl Default for EncodeSettings {
    fn default() -> Self {
        Self {
            codec: Codec::H264,
            hardware: true,
            adapter_luid: None,
            quality: Some(70),
            bits_per_pixel_frame: 0.10,
            gop_frames: 20,
            screen_hint: true,
            low_latency: false,
            zstd_level: -3,
            raw_waiting: false,
        }
    }
}

static SETTINGS: OnceLock<EncodeSettings> = OnceLock::new();

/// Whether the encoder accepted `CODECAPI_AVLowLatencyMode`: −1 not asked.
pub static LOW_LATENCY_ACCEPTED: AtomicI8 = AtomicI8::new(-1);

thread_local! {
    /// The DXGI device manager of the run's adapter, for the Sink Writer.
    static D3D_MANAGER: RefCell<Option<IMFDXGIDeviceManager>> = const { RefCell::new(None) };
}

/// Installs the run's settings; the first call wins.
pub fn install(settings: EncodeSettings) {
    let _ = SETTINGS.set(settings);
}

/// The run's settings, or the product's values when none were installed.
pub fn settings() -> &'static EncodeSettings {
    SETTINGS.get_or_init(EncodeSettings::default)
}

pub fn set_d3d_manager(manager: Option<IMFDXGIDeviceManager>) {
    D3D_MANAGER.with(|slot| *slot.borrow_mut() = manager);
}

pub fn d3d_manager() -> Option<IMFDXGIDeviceManager> {
    D3D_MANAGER.with(|slot| slot.borrow().clone())
}
