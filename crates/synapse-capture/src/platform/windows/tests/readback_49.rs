//! Readback validation tests (moved out of `capture.rs`, #49).

use windows::Win32::Graphics::{
    Direct3D11::D3D11_TEXTURE2D_DESC,
    Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
    },
};

use super::{bytes_per_pixel_for_format, validate_pixel_format, validate_readback_source};
use crate::{CaptureError, DxgiFormat};

fn desc_with(format: DXGI_FORMAT, sample_count: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: 64,
        Height: 48,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: sample_count,
            Quality: 0,
        },
        ..Default::default()
    }
}

#[test]
fn must_fail_readback_rejects_multisampled_source() {
    let desc = desc_with(DXGI_FORMAT_B8G8R8A8_UNORM, 4);

    let error = validate_readback_source(&desc, 64, 48, 11).unwrap_err();
    assert!(matches!(error, CaptureError::GraphicsApiUnsupported { .. }));
    assert!(error.to_string().contains("multisampled"));
}

#[test]
fn must_pass_readback_cache_key_tracks_dimensions_and_native_format() -> Result<(), CaptureError> {
    let desc = desc_with(DXGI_FORMAT_B8G8R8A8_UNORM, 1);

    let key = validate_readback_source(&desc, 64, 48, 11)?;
    assert_eq!(key.device_identity, 11);
    assert_eq!(key.width, 64);
    assert_eq!(key.height, 48);
    assert_eq!(key.format, DXGI_FORMAT_B8G8R8A8_UNORM);
    Ok(())
}

#[test]
fn must_pass_readback_cache_key_changes_with_device_identity() -> Result<(), CaptureError> {
    let desc = desc_with(DXGI_FORMAT_B8G8R8A8_UNORM, 1);

    let first = validate_readback_source(&desc, 64, 48, 11)?;
    let replacement = validate_readback_source(&desc, 64, 48, 12)?;
    assert_ne!(first, replacement);
    Ok(())
}

#[test]
fn must_fail_transport_rejects_formats_that_ocr_cannot_interpret() {
    for format in [
        DxgiFormat::Rgba16F,
        DxgiFormat::Rgb10A2,
        DxgiFormat::Rgb10XrA2,
    ] {
        let error = bytes_per_pixel_for_format(format).unwrap_err();
        assert!(matches!(error, CaptureError::GraphicsApiUnsupported { .. }));
        assert!(error.to_string().contains("explicit conversion"));
    }
}

#[test]
fn must_fail_claimed_pixel_format_must_match_texture_format() -> Result<(), CaptureError> {
    validate_pixel_format(&desc_with(DXGI_FORMAT_B8G8R8A8_UNORM, 1), DxgiFormat::Bgra8)?;
    validate_pixel_format(
        &desc_with(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, 1),
        DxgiFormat::Bgra8Srgb,
    )?;
    validate_pixel_format(&desc_with(DXGI_FORMAT_R8G8B8A8_UNORM, 1), DxgiFormat::Rgba8)?;

    // A 4-byte claim over an 8-byte-per-pixel texture would under-read every row.
    let wide = desc_with(DXGI_FORMAT_R16G16B16A16_FLOAT, 1);
    let error = validate_pixel_format(&wide, DxgiFormat::Bgra8).unwrap_err();
    assert!(matches!(error, CaptureError::GraphicsApiUnsupported { .. }));
    assert!(error.to_string().contains("claimed format"));

    // Same width, wrong channel order / transfer function.
    for (texture, claimed) in [
        (DXGI_FORMAT_R8G8B8A8_UNORM, DxgiFormat::Bgra8),
        (DXGI_FORMAT_B8G8R8A8_UNORM, DxgiFormat::Rgba8),
        (DXGI_FORMAT_B8G8R8A8_UNORM, DxgiFormat::Bgra8Srgb),
        (DXGI_FORMAT_B8G8R8A8_UNORM, DxgiFormat::Unknown(7)),
    ] {
        let error = match validate_pixel_format(&desc_with(texture, 1), claimed) {
            Ok(()) => panic!("claim {claimed:?} over texture {texture:?} must be refused"),
            Err(error) => error,
        };
        assert!(
            matches!(error, CaptureError::GraphicsApiUnsupported { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("claimed format"), "{error}");
    }
    Ok(())
}
