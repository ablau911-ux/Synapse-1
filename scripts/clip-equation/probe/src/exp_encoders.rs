//! Experiments step 1a (docs/clip-recorder-experiments-plan.md, section
//! "Factors", first paragraph): every Media Foundation video encoder on this
//! PC for H.264, HEVC and AV1, hardware and software (`MFT_ENUM_FLAG_ALL`).
//!
//! `clip-probe exp-encoders` prints one line per encoder and writes
//! `scripts/clip-equation/results/encoders.json`.

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, MF_TRANSFORM_FLAGS_Attribute, MFMediaType_Video, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_ADAPTER_LUID, MFT_ENUM_FLAG_ALL, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_HARDWARE_URL_Attribute, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFT_REGISTER_TYPE_INFO, MFT_TRANSFORM_CLSID_Attribute, MFTEnumEx,
    MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::GUID;

use crate::Result;
use crate::encoder::{MediaFoundation, attribute_string};

/// One encoder as Media Foundation lists it.
struct EncoderEntry {
    codec: &'static str,
    name: String,
    vendor_id: String,
    hardware_url: String,
    hardware: bool,
    flags: u32,
    clsid: String,
    adapter_luid: String,
}

pub fn run() -> Result<()> {
    let _media = MediaFoundation::start()?;
    let mut entries = Vec::new();
    for (codec, subtype) in [
        ("H.264", MFVideoFormat_H264),
        ("HEVC", MFVideoFormat_HEVC),
        ("AV1", MFVideoFormat_AV1),
    ] {
        entries.extend(enumerate(codec, subtype)?);
    }
    for entry in &entries {
        println!(
            "exp-encoders codec={} hardware={} name=\"{}\" vendor_id=\"{}\" clsid={} flags={:#x} adapter_luid={} url=\"{}\"",
            entry.codec,
            entry.hardware,
            entry.name,
            entry.vendor_id,
            entry.clsid,
            entry.flags,
            entry.adapter_luid,
            entry.hardware_url
        );
    }
    let path = results_folder()?.join("encoders.json");
    std::fs::write(&path, to_json(&entries))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    println!(
        "exp-encoders count={} written={}",
        entries.len(),
        path.display()
    );
    Ok(())
}

/// `scripts/clip-equation/results`, created when missing.
pub(crate) fn results_folder() -> Result<PathBuf> {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no scripts/clip-equation folder")?
        .join("results");
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("create {}: {error}", folder.display()))?;
    Ok(folder)
}

fn enumerate(codec: &'static str, subtype: GUID) -> Result<Vec<EncoderEntry>> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0_u32;
    // SAFETY: out-parameters for an array that is freed below.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_ALL,
            None,
            Some(&raw const output),
            &raw mut activates,
            &raw mut count,
        )
    }
    .map_err(|error| format!("MFTEnumEx {codec} failed: {error}"))?;
    if activates.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: MFTEnumEx returned `count` entries at `activates`.
    let list = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let entries = list
        .iter_mut()
        .filter_map(Option::take)
        .map(|activate| {
            // SAFETY: plain attribute reads on an activation object.
            let flags = unsafe { activate.GetUINT32(&MF_TRANSFORM_FLAGS_Attribute) }.unwrap_or(0);
            // SAFETY: as above.
            let clsid = unsafe { activate.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) }
                .map_or_else(|_| String::new(), |guid| format!("{{{guid:?}}}"));
            // SAFETY: as above. Hardware encoders are listed once per adapter.
            let adapter_luid = unsafe { activate.GetUINT64(&MFT_ENUM_ADAPTER_LUID) }
                .map_or_else(|_| String::new(), |luid| format!("{luid:#x}"));
            EncoderEntry {
                codec,
                adapter_luid,
                name: attribute_string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default(),
                vendor_id: attribute_string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
                    .unwrap_or_default(),
                hardware_url: attribute_string(&activate, &MFT_ENUM_HARDWARE_URL_Attribute)
                    .unwrap_or_default(),
                hardware: flags & MFT_ENUM_FLAG_HARDWARE.0 as u32 != 0,
                flags,
                clsid,
            }
        })
        .collect();
    // SAFETY: the array MFTEnumEx allocated; its entries were released above.
    unsafe { CoTaskMemFree(Some(activates.cast::<c_void>().cast_const())) };
    Ok(entries)
}

fn to_json(entries: &[EncoderEntry]) -> String {
    let rows: Vec<String> = entries
        .iter()
        .map(|e| {
            format!(
                "  {{\"codec\": {}, \"name\": {}, \"vendor_id\": {}, \"hardware_url\": {}, \"hardware\": {}, \"transform_flags\": {}, \"clsid\": {}, \"adapter_luid\": {}}}",
                json_string(e.codec),
                json_string(&e.name),
                json_string(&e.vendor_id),
                json_string(&e.hardware_url),
                e.hardware,
                e.flags,
                json_string(&e.clsid),
                json_string(&e.adapter_luid)
            )
        })
        .collect();
    format!("[\n{}\n]\n", rows.join(",\n"))
}

/// A JSON string literal.
pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
