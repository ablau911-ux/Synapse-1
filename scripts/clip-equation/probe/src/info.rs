//! PC facts for the results file, and the check that the cycle counters
//! measure processor time at a fixed, known rate.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
use windows::core::PCWSTR;
use windows_capture::graphics_capture_api::GraphicsCaptureApi;

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_MS, HNS_PER_SECOND};
use crate::gpu::Gpu;
use crate::load::BusyLoad;

pub fn run() -> Result<()> {
    let threads = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    println!("logical_processors={threads}");

    let rate = CycleRate::calibrate();
    println!("cycle_counter_hz={:.0}", rate.per_second());
    // A 2 s spin read three ways: wall time, cycle counter, GetThreadTimes.
    let (cycles_start, times_start, wall_start) = (
        clock::thread_cycles(),
        clock::current_thread_time_hns(),
        clock::now_hns(),
    );
    while clock::now_hns() - wall_start < 2 * HNS_PER_SECOND {
        std::hint::spin_loop();
    }
    let (cycles_end, times_end, wall_end) = (
        clock::thread_cycles(),
        clock::current_thread_time_hns(),
        clock::now_hns(),
    );
    println!(
        "clock_check spin_wall_s={:.4} cycle_counter_s={:.4} get_thread_times_s={:.4}",
        (wall_end - wall_start) as f64 / HNS_PER_SECOND as f64,
        rate.seconds(cycles_end - cycles_start),
        (times_end - times_start) as f64 / HNS_PER_SECOND as f64
    );
    // A fixed-rate counter reads the same with every core busy (lower clocks)
    // and in short bursts between sleeps (clock ramps); a core-clock counter would not.
    let loaded = {
        let _load = BusyLoad::start(threads.saturating_sub(1));
        CycleRate::calibrate()
    };
    println!(
        "clock_check cycle_counter_hz_all_cores_busy={:.0}",
        loaded.per_second()
    );
    let (mut burst_wall, mut burst_cycles) = (0_i64, 0_u64);
    for _ in 0..400 {
        let (cycles, wall) = (clock::thread_cycles(), clock::now_hns());
        while clock::now_hns() - wall < HNS_PER_MS {
            std::hint::spin_loop();
        }
        burst_cycles += clock::thread_cycles() - cycles;
        burst_wall += clock::now_hns() - wall;
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    println!(
        "clock_check bursts(400 x 1 ms spin, 4 ms sleep) wall_s={:.4} cycle_counter_s={:.4}",
        burst_wall as f64 / HNS_PER_SECOND as f64,
        rate.seconds(burst_cycles)
    );

    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: a DEVMODEW with its size set.
    if unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &raw mut mode) }
        .as_bool()
    {
        println!(
            "display {}x{} refresh_hz={}",
            mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency
        );
    }
    let mut timing = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a DWM_TIMING_INFO with its size set; a null window means the desktop.
    if unsafe { DwmGetCompositionTimingInfo(HWND::default(), &raw mut timing) }.is_ok() {
        // DWM_TIMING_INFO is packed: copy each field out before formatting it.
        let (refresh_num, refresh_den) = (
            timing.rateRefresh.uiNumerator,
            timing.rateRefresh.uiDenominator,
        );
        let (compose_num, compose_den) = (
            timing.rateCompose.uiNumerator,
            timing.rateCompose.uiDenominator,
        );
        println!(
            "dwm_refresh_rate={refresh_num}/{refresh_den} composition_rate={compose_num}/{compose_den}"
        );
    }

    for gpu in Gpu::hardware_adapters()? {
        println!("adapter=\"{}\"", gpu.adapter);
    }
    println!(
        "default_adapter(windows-capture device)=\"{}\"",
        Gpu::default_adapter_name()?
    );

    let flag = |supported: std::result::Result<bool, _>| match supported {
        Ok(value) => value.to_string(),
        Err(error) => format!("error({error:?})"),
    };
    println!(
        "graphics_capture supported={} cursor={} border={} secondary_windows={} minimum_update_interval={} dirty_regions={}",
        flag(GraphicsCaptureApi::is_supported()),
        flag(GraphicsCaptureApi::is_cursor_settings_supported()),
        flag(GraphicsCaptureApi::is_border_settings_supported()),
        flag(GraphicsCaptureApi::is_secondary_windows_supported()),
        flag(GraphicsCaptureApi::is_minimum_update_interval_supported()),
        flag(GraphicsCaptureApi::is_dirty_region_supported()),
    );
    Ok(())
}
