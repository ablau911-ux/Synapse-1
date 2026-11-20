//! D3D11 staging-texture readback shared by the WGC and DXGI capture loops.

use std::{slice, thread};

use windows::{
    Win32::Graphics::{
        Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_STAGING, ID3D11Resource, ID3D11Texture2D,
        },
        Dxgi::Common::{
            DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_UNORM,
            DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM,
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        },
    },
    core::Interface as _,
};

use super::common::capture_unsupported;
use crate::{CaptureError, CapturedFrameBuffer, DxgiFormat};

#[derive(Default)]
pub(super) struct TextureReadback {
    staging: Option<ID3D11Texture2D>,
    staging_key: Option<StagingKey>,
    owner_thread: Option<thread::ThreadId>,
}

impl Drop for TextureReadback {
    fn drop(&mut self) {
        debug_assert!(
            self.staging.is_none() || self.owner_thread == Some(thread::current().id()),
            "D3D11 staging cache crossed threads without producer-thread release"
        );
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct StagingKey {
    device_identity: usize,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl TextureReadback {
    pub(super) fn copy_texture_to_owned_buffer(
        &mut self,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
        format: DxgiFormat,
    ) -> Result<CapturedFrameBuffer, CaptureError> {
        let bytes_per_pixel = bytes_per_pixel_for_format(format)?;
        let desc = texture_desc(texture);
        validate_pixel_format(&desc, format)?;
        // SAFETY: `texture` is a live COM reference borrowed for this call; GetDevice only
        // adds a reference to the owning device and writes no caller memory.
        let device = unsafe { texture.GetDevice() }.map_err(capture_unsupported)?;
        let device_identity = device.as_raw() as usize;
        let key = validate_readback_source(&desc, width, height, device_identity)?;
        if self.staging_key != Some(key) {
            self.release_on_owner_thread();
            self.staging = Some(create_full_frame_staging_texture(texture, desc)?);
            self.staging_key = Some(key);
            self.owner_thread = Some(thread::current().id());
        }
        let staging =
            self.staging
                .as_ref()
                .ok_or_else(|| CaptureError::GraphicsApiUnsupported {
                    detail: "capture readback cache did not contain a staging texture".to_owned(),
                })?;
        // SAFETY: `device` was obtained above and is kept alive by this scope; the call only
        // returns a new reference to the device's immediate context.
        let context = unsafe { device.GetImmediateContext() }.map_err(capture_unsupported)?;
        let source: ID3D11Resource = texture.cast().map_err(capture_unsupported)?;
        let target: ID3D11Resource = staging.cast().map_err(capture_unsupported)?;
        // The immediate context is not thread-safe. Every `TextureReadback` is created and
        // used inside one producer call chain (WGC: callback-local; DXGI: the capture loop),
        // and its `owner_thread` debug assertions check the cached staging texture never
        // crosses threads, so the context is only touched from that single thread.
        // SAFETY: `target` (staging, created from `source`'s desc with identical
        // dimensions/format, validated above) and `source` are live COM resources on
        // `device`; subresource 0 exists on both (MipLevels = ArraySize = 1 for staging
        // and the source is a single 2D surface); `None` copies the whole subresource.
        unsafe {
            context.CopySubresourceRegion(&target, 0, 0, 0, 0, &source, 0, None);
        }
        // SAFETY: `device` is a live ID3D11Device; the call has no pointer arguments.
        unsafe { device.GetDeviceRemovedReason() }.map_err(capture_unsupported)?;

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: `target` is a D3D11_USAGE_STAGING texture with CPU read access, and
        // `mapped` is a valid out-pointer for the duration of the call. On success D3D
        // guarantees `pData` addresses `RowPitch * Height` readable bytes until Unmap.
        unsafe { context.Map(&target, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
            .map_err(capture_unsupported)?;
        let bytes = copy_mapped_rows_tight(&mapped, width, height, usize::from(bytes_per_pixel));
        // SAFETY: subresource 0 of `target` was mapped above and no slice into the mapping
        // outlives `copy_mapped_rows_tight`, which returned owned bytes.
        unsafe { context.Unmap(&target, 0) };
        let bytes = bytes?;
        Ok(CapturedFrameBuffer {
            bytes,
            row_stride_bytes: frame_row_len(width, usize::from(bytes_per_pixel))?,
            bytes_per_pixel,
        })
    }

    fn release_on_owner_thread(&mut self) {
        debug_assert!(
            self.owner_thread.is_none() || self.owner_thread == Some(thread::current().id()),
            "D3D11 staging cache release must run on its producer thread"
        );
        self.staging = None;
        self.staging_key = None;
        self.owner_thread = None;
    }
}

fn create_full_frame_staging_texture(
    texture: &ID3D11Texture2D,
    mut desc: D3D11_TEXTURE2D_DESC,
) -> Result<ID3D11Texture2D, CaptureError> {
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0.cast_unsigned();
    desc.MiscFlags = 0;

    // SAFETY: `texture` is a live COM reference; GetDevice only returns a new reference.
    let device = unsafe { texture.GetDevice() }.map_err(capture_unsupported)?;
    let mut staging = None;
    // SAFETY: `desc` is a fully initialised descriptor on the stack and `staging` is a
    // valid out-slot, both live across the call; no initial data is passed (`None`).
    unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut staging)) }
        .map_err(capture_unsupported)?;
    staging.ok_or_else(|| CaptureError::GraphicsApiUnsupported {
        detail: "CreateTexture2D returned no owned staging texture".to_owned(),
    })
}

fn validate_readback_source(
    desc: &D3D11_TEXTURE2D_DESC,
    width: u32,
    height: u32,
    device_identity: usize,
) -> Result<StagingKey, CaptureError> {
    if desc.Width != width || desc.Height != height {
        return Err(CaptureError::GraphicsApiUnsupported {
            detail: format!(
                "capture texture dimensions {}x{} did not match frame dimensions {width}x{height}",
                desc.Width, desc.Height
            ),
        });
    }
    if desc.SampleDesc.Count != 1 || desc.SampleDesc.Quality != 0 {
        return Err(CaptureError::GraphicsApiUnsupported {
            detail: format!(
                "multisampled capture textures are unsupported for CPU readback: sample_count={} sample_quality={}",
                desc.SampleDesc.Count, desc.SampleDesc.Quality
            ),
        });
    }
    Ok(StagingKey {
        device_identity,
        width,
        height,
        format: desc.Format,
    })
}

/// Rejects a frame whose claimed pixel format differs from the texture's real format.
///
/// `copy_mapped_rows_tight` sizes its reads from the claimed bytes-per-pixel, so a
/// mismatch would read the wrong number of bytes per row (or silently swap channels).
fn validate_pixel_format(
    desc: &D3D11_TEXTURE2D_DESC,
    format: DxgiFormat,
) -> Result<(), CaptureError> {
    let accepted: &[DXGI_FORMAT] = match format {
        DxgiFormat::Bgra8 => &[
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_FORMAT_B8G8R8A8_TYPELESS,
            DXGI_FORMAT_B8G8R8X8_UNORM,
        ],
        DxgiFormat::Bgra8Srgb => &[DXGI_FORMAT_B8G8R8A8_UNORM_SRGB],
        DxgiFormat::Rgba8 => &[DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_TYPELESS],
        DxgiFormat::Rgba8Srgb => &[DXGI_FORMAT_R8G8B8A8_UNORM_SRGB],
        // Not CPU-readable as 8-bit pixels; bytes_per_pixel_for_format rejects these.
        DxgiFormat::Rgba16F
        | DxgiFormat::Rgb10A2
        | DxgiFormat::Rgb10XrA2
        | DxgiFormat::Unknown(_) => &[],
    };
    if accepted.contains(&desc.Format) {
        Ok(())
    } else {
        Err(CaptureError::GraphicsApiUnsupported {
            detail: format!(
                "capture frame claimed format {format:?} but the texture format is DXGI {}",
                desc.Format.0
            ),
        })
    }
}

fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid, writable D3D11_TEXTURE2D_DESC for the duration of the call.
    unsafe {
        texture.GetDesc(&raw mut desc);
    }
    desc
}

fn copy_mapped_rows_tight(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>, CaptureError> {
    if mapped.pData.is_null() {
        return Err(CaptureError::GraphicsApiUnsupported {
            detail: "capture frame Map returned null pData".to_owned(),
        });
    }
    let row_len = frame_row_len(width, bytes_per_pixel)?;
    let row_pitch =
        usize::try_from(mapped.RowPitch).map_err(|err| CaptureError::GraphicsApiUnsupported {
            detail: format!("invalid capture frame row pitch {}: {err}", mapped.RowPitch),
        })?;
    if row_pitch < row_len {
        return Err(CaptureError::GraphicsApiUnsupported {
            detail: format!(
                "capture frame row pitch {row_pitch} is smaller than row length {row_len}"
            ),
        });
    }
    let height = usize::try_from(height).map_err(|err| CaptureError::TargetInvalid {
        detail: err.to_string(),
    })?;
    let byte_len = row_len
        .checked_mul(height)
        .ok_or_else(|| CaptureError::TargetInvalid {
            detail: format!("invalid capture frame dimensions {width}x{height}"),
        })?;
    let mut bytes = Vec::with_capacity(byte_len);
    let base = mapped.pData.cast::<u8>();
    for row in 0..height {
        let row_offset = row
            .checked_mul(row_pitch)
            .ok_or_else(|| CaptureError::TargetInvalid {
                detail: format!("invalid capture frame row offset row={row} pitch={row_pitch}"),
            })?;
        // SAFETY: `pData` is non-null (checked above) and, per the Map contract, addresses
        // `RowPitch * Height` readable bytes for a texture whose height equals `height`
        // (validate_readback_source). `row < height` and `row_pitch >= row_len` (checked
        // above), so `row_offset + row_len <= (height - 1) * row_pitch + row_len` stays
        // inside the mapping; `row_offset` was computed with checked_mul. u8 has
        // alignment 1, and the staging mapping is CPU-read only and unmapped only after
        // this function returns owned bytes.
        let source = unsafe { slice::from_raw_parts(base.add(row_offset), row_len) };
        bytes.extend_from_slice(source);
    }
    Ok(bytes)
}

fn frame_row_len(width: u32, bytes_per_pixel: usize) -> Result<usize, CaptureError> {
    usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(bytes_per_pixel))
        .ok_or_else(|| CaptureError::TargetInvalid {
            detail: format!("invalid capture frame width {width}"),
        })
}

fn bytes_per_pixel_for_format(format: DxgiFormat) -> Result<u8, CaptureError> {
    match format {
        DxgiFormat::Bgra8 | DxgiFormat::Bgra8Srgb | DxgiFormat::Rgba8 | DxgiFormat::Rgba8Srgb => {
            Ok(4)
        }
        DxgiFormat::Rgba16F | DxgiFormat::Rgb10A2 | DxgiFormat::Rgb10XrA2 => {
            Err(CaptureError::GraphicsApiUnsupported {
                detail: format!(
                    "capture frame format {format:?} requires explicit conversion to 8-bit BGRA/RGBA"
                ),
            })
        }
        DxgiFormat::Unknown(value) => Err(CaptureError::GraphicsApiUnsupported {
            detail: format!("capture frame copy does not support unknown DXGI format {value}"),
        }),
    }
}

#[cfg(test)]
#[path = "tests/readback_49.rs"]
mod tests;
