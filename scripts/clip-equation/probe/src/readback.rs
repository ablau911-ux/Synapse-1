//! M1: the real read-back cost (plan F16, symbol c_read). A GPU texture shaped
//! like a capture frame is copied into a staging texture, mapped, and its rows
//! are copied tightly into processor memory; at 854 x 480, 1280 x 720 and
//! 1920 x 1080, on every hardware adapter.
//!
//! Three ways are timed: a reused staging texture with a blocking `Map`, the
//! same with "do not wait" and retry, and a new staging texture per frame (what
//! the screenshot path does today, plan F1).

use crate::Result;
use crate::clock::{self, CycleRate};
use crate::gpu::{self, Gpu, MapWait, ReadBack};
use crate::stats::Summary;

const SIZES: [(u32, u32); 3] = [(854, 480), (1280, 720), (1920, 1080)];
const WARMUP: usize = 10;
const ROUNDS: usize = 120;

#[derive(Clone, Copy)]
enum Way {
    ReusedStaging(MapWait),
    NewStagingPerFrame,
}

impl Way {
    fn name(self) -> String {
        match self {
            Self::ReusedStaging(wait) => format!("reused_staging+{}", wait.name()),
            Self::NewStagingPerFrame => "new_staging_per_frame+blocking_map".to_owned(),
        }
    }
}

pub fn run() -> Result<()> {
    let rate = CycleRate::calibrate();
    println!("cycle_counter_hz={:.0}", rate.per_second());
    println!("default_adapter=\"{}\"", Gpu::default_adapter_name()?);
    for gpu in Gpu::hardware_adapters()? {
        for (width, height) in SIZES {
            let source = gpu::frame_texture(&gpu.device, width, height)?;
            gpu::fill_pattern(&gpu.context, &source, width, height);
            let ways = [
                Way::ReusedStaging(MapWait::Block),
                Way::ReusedStaging(MapWait::PollYield),
                Way::ReusedStaging(MapWait::PollSleep),
                Way::NewStagingPerFrame,
            ];
            for way in ways {
                measure(&gpu, &source, (width, height), way, rate)?;
            }
        }
    }
    Ok(())
}

fn measure(
    gpu: &Gpu,
    source: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    size: (u32, u32),
    way: Way,
    rate: CycleRate,
) -> Result<()> {
    let (width, height) = size;
    let mut readback = ReadBack::new(&gpu.device, width, height)?;
    let (mut wall_ms, mut cpu_ms) = (Vec::with_capacity(ROUNDS), Vec::with_capacity(ROUNDS));
    let mut retries = 0_u64;
    let mut process_start = 0;
    for round in 0..WARMUP + ROUNDS {
        if round == WARMUP {
            process_start = clock::process_cycles();
        }
        let (wall_start, cycles_start) = (clock::now_hns(), clock::thread_cycles());
        let tries = match way {
            Way::ReusedStaging(wait) => readback.read(&gpu.context, source, wait)?,
            Way::NewStagingPerFrame => {
                readback = ReadBack::new(&gpu.device, width, height)?;
                readback.read(&gpu.context, source, MapWait::Block)?
            }
        };
        let (wall_end, cycles_end) = (clock::now_hns(), clock::thread_cycles());
        if round >= WARMUP {
            wall_ms.push(clock::hns_to_ms(wall_end - wall_start));
            cpu_ms.push(rate.millis(cycles_end - cycles_start));
            retries += u64::from(tries);
        }
    }
    let process_s = rate.seconds(clock::process_cycles() - process_start);
    let pixels = f64::from(width) * f64::from(height);
    let thread = Summary::of(&cpu_ms);
    println!(
        "M1 adapter=\"{}\" size={width}x{height} way={} wall_ms {} | thread_cpu_ms {} | thread_cpu_ns_per_px={:.4} process_cpu_ns_per_px={:.4} map_retries_per_read={:.1}",
        gpu.adapter,
        way.name(),
        Summary::of(&wall_ms),
        thread,
        thread.mean * 1e6 / pixels,
        process_s * 1e9 / (ROUNDS as f64 * pixels),
        retries as f64 / ROUNDS as f64
    );
    Ok(())
}
