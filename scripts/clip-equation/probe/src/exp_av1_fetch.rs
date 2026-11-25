//! exp-av1-fetch: fetch the picture of one instant from a clip folder through the
//! PRODUCT's decoder (`crate::clip_decoder::ClipDecoder::picture_at`, the call the viewer
//! makes), with a fresh decoder each run.
//!
//!   clip-probe exp-av1-fetch folder=<clip folder> instant=<k> [png=<file>]
//!
//! Prints one JSON line: instant, kind, shown_instant, width, height, read_part_ms,
//! decode_ms, convert_ms, total_ms (opening the folder to having the RGBA picture),
//! pictures_decoded, rgba_sha256 (SHA-256 of the returned RGBA pixels). A lost instant has
//! no picture: width, height and rgba_sha256 are null and no PNG is written.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::Digest as _;

use crate::Result;
use crate::clip_decoder::ClipDecoder;
use crate::clip_folder::read_index;
use crate::exp_meter::Args;

const NS_PER_MS: f64 = 1_000_000.0;

fn ms(time: Duration) -> f64 {
    time.as_nanos() as f64 / NS_PER_MS
}

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let folder = PathBuf::from(args.text("folder")?);
    let instant: u64 = args
        .text("instant")?
        .parse()
        .map_err(|_| "instant=<k> must be an instant number".to_owned())?;
    let png = args.text("png").ok().map(PathBuf::from);
    let started = Instant::now();
    let lines = read_index(&folder).map_err(|e| format!("read {}: {e}", folder.display()))?;
    let mut decoder = ClipDecoder::new(&folder, lines);
    let picture = decoder.picture_at(instant)?;
    let total = started.elapsed();
    let times = decoder.times();
    let image = picture.image.as_ref();
    let sha256 = image.map(|image| {
        sha2::Sha256::digest(image.as_raw())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    let line = json!({
        "instant": picture.instant,
        "kind": picture.kind().as_str(),
        "shown_instant": picture.source.shown_instant(),
        "width": image.map(image::RgbaImage::width),
        "height": image.map(image::RgbaImage::height),
        "read_part_ms": ms(times.read_part),
        "decode_ms": ms(times.decode),
        "convert_ms": ms(times.convert),
        "total_ms": ms(total),
        "pictures_decoded": times.pictures_decoded,
        "rgba_sha256": sha256
    });
    println!("{line}");
    if let (Some(path), Some(image)) = (png, image) {
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}
