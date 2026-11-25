//! Direct3D 11 pieces the probes share: a device on a chosen adapter,
//! textures shaped like Windows capture frames, and the read-back the clip
//! recorder will do (copy into a staging texture, `Map`, tight row copy).

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_WAS_STILL_DRAWING, IDXGIAdapter,
    IDXGIDevice, IDXGIFactory1,
};
use windows::core::Interface;

use crate::{Result, win};

/// Lets a Direct3D object cross threads. Direct3D 11 devices are
/// free-threaded; the immediate context is shared between threads only after
/// multithread protection is switched on (`slots.rs`).
pub struct SendCom<T>(pub T);
// SAFETY: see the type comment.
unsafe impl<T> Send for SendCom<T> {}
// SAFETY: see the type comment.
unsafe impl<T> Sync for SendCom<T> {}

/// A device and its immediate context on one adapter.
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter: String,
}

impl Gpu {
    /// One device per hardware adapter, in DXGI order; software rasterizers are skipped.
    pub fn hardware_adapters() -> Result<Vec<Self>> {
        // SAFETY: plain factory creation.
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.map_err(win("CreateDXGIFactory1"))?;
        let mut gpus = Vec::new();
        for index in 0.. {
            // SAFETY: enumeration ends with an error past the last adapter.
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
                break;
            };
            // SAFETY: plain query.
            let desc = unsafe { adapter.GetDesc1() }.map_err(win("IDXGIAdapter1::GetDesc1"))?;
            if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let adapter: IDXGIAdapter = adapter.cast().map_err(win("cast IDXGIAdapter"))?;
            let (device, context) = create_device(Some(&adapter))?;
            gpus.push(Self {
                device,
                context,
                adapter: utf16_name(&desc.Description),
            });
        }
        Ok(gpus)
    }

    /// The adapter a device lands on when no adapter is named, which is what
    /// windows-capture's `create_d3d_device` does for every capture.
    pub fn default_adapter_name() -> Result<String> {
        let (device, _) = create_device(None)?;
        Ok(adapter_name(&device))
    }
}

fn create_device(adapter: Option<&IDXGIAdapter>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let driver = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    let (mut device, mut context) = (None, None);
    // SAFETY: out-parameters for the device and its context.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(win("D3D11CreateDevice"))?;
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err("D3D11CreateDevice returned no device".to_owned()),
    }
}

/// The adapter name of a device, as DXGI reports it.
pub fn adapter_name(device: &ID3D11Device) -> String {
    let name = || -> windows::core::Result<String> {
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: plain queries.
        let desc = unsafe { dxgi.GetAdapter()?.GetDesc()? };
        Ok(utf16_name(&desc.Description))
    };
    name().unwrap_or_else(|error| format!("unknown ({error})"))
}

fn utf16_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

fn texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    usage: D3D11_USAGE,
) -> Result<ID3D11Texture2D> {
    let staging = usage == D3D11_USAGE_STAGING;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: usage,
        BindFlags: if staging {
            0
        } else {
            (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut created = None;
    // SAFETY: a valid description and one out-parameter.
    unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut created)) }
        .map_err(win("ID3D11Device::CreateTexture2D"))?;
    created.ok_or_else(|| "CreateTexture2D returned no texture".to_owned())
}

/// A GPU texture shaped like a Windows capture frame (BGRA, render target).
pub fn frame_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    texture(device, width, height, D3D11_USAGE_DEFAULT)
}

/// A texture the processor can map for reading.
pub fn staging_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    texture(device, width, height, D3D11_USAGE_STAGING)
}

/// Fills a frame texture with a fixed pattern, so a read-back moves real bytes.
pub fn fill_pattern(
    context: &ID3D11DeviceContext,
    target: &ID3D11Texture2D,
    width: u32,
    height: u32,
) {
    let pixels: Vec<u8> = (0..width as usize * height as usize * 4)
        .map(|index| ((index * 31 + index / 7) % 251) as u8)
        .collect();
    // SAFETY: `pixels` holds height rows of width * 4 bytes.
    unsafe {
        context.UpdateSubresource(
            target,
            0,
            None,
            pixels.as_ptr().cast::<c_void>(),
            width * 4,
            0,
        );
    }
}

/// How the processor waits for the GPU copy before reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapWait {
    /// `Map` without flags: the call waits inside the driver.
    Block,
    /// `Map` with `D3D11_MAP_FLAG_DO_NOT_WAIT`; on "still drawing" the thread yields and retries.
    PollYield,
    /// `Map` with `D3D11_MAP_FLAG_DO_NOT_WAIT`; on "still drawing" the thread sleeps 1 ms and retries.
    PollSleep,
}

impl MapWait {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Block => "blocking_map",
            Self::PollYield => "do_not_wait_yield_retry",
            Self::PollSleep => "do_not_wait_sleep1ms_retry",
        }
    }
}

/// Maps a staging texture for reading. Returns the mapping and the number of
/// "still drawing" answers before it succeeded.
pub fn map_for_read(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    wait: MapWait,
) -> Result<(D3D11_MAPPED_SUBRESOURCE, u32)> {
    match wait {
        MapWait::Block => {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: a staging texture created with CPU read access.
            unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
                .map_err(win("ID3D11DeviceContext::Map"))?;
            Ok((mapped, 0))
        }
        MapWait::PollYield | MapWait::PollSleep => {
            let mut retries = 0;
            loop {
                if let Some(mapped) = try_map_for_read(context, staging)? {
                    return Ok((mapped, retries));
                }
                retries += 1;
                if wait == MapWait::PollSleep {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                } else {
                    std::thread::yield_now();
                }
            }
        }
    }
}

/// One `Map` attempt with "do not wait"; None while the GPU is still drawing.
pub fn try_map_for_read(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
) -> Result<Option<D3D11_MAPPED_SUBRESOURCE>> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    let flags = D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32;
    // SAFETY: a staging texture created with CPU read access.
    match unsafe { context.Map(staging, 0, D3D11_MAP_READ, flags, Some(&raw mut mapped)) } {
        Ok(()) => Ok(Some(mapped)),
        Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => Ok(None),
        Err(error) => Err(format!(
            "ID3D11DeviceContext::Map (do not wait) failed: {error}"
        )),
    }
}

/// One frame read back to the processor into a tightly packed BGRA buffer.
pub struct ReadBack {
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl ReadBack {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            staging: staging_texture(device, width, height)?,
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        })
    }

    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Copies `source` into the staging texture on the GPU, maps it and copies
    /// its rows into `pixels` without the row padding. Returns the number of
    /// "still drawing" answers `Map` gave first.
    pub fn read(
        &mut self,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        wait: MapWait,
    ) -> Result<u32> {
        // SAFETY: same size and format; the GPU copy is ordered before the Map.
        unsafe { context.CopyResource(&self.staging, source) };
        let (mapped, retries) = map_for_read(context, &self.staging, wait)?;
        let row_bytes = self.width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        for (row, target) in self.pixels.chunks_exact_mut(row_bytes).enumerate() {
            // SAFETY: Map returned `height` rows spaced `RowPitch` apart, each at
            // least `row_bytes` long, valid until Unmap.
            let source_row = unsafe {
                std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(row * pitch), row_bytes)
            };
            target.copy_from_slice(source_row);
        }
        // SAFETY: mapped above.
        unsafe { context.Unmap(&self.staging, 0) };
        Ok(retries)
    }
}
