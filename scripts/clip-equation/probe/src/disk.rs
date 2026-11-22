//! M5: raw write volume and its processor cost (symbol c_disk, owner figure
//! E8). 1920 x 1080 BGRA frames (8,294,400 bytes) are written as one file per
//! frame into a temporary folder on the %LOCALAPPDATA% volume:
//!
//!   buffered        through the file cache, as fast as possible (the product's way)
//!   write_through   no cache, write-through: what the disk itself sustains
//!   paced_20fps     through the cache at 20 frames per second (166 MB/s), the
//!                   worst-case live rate; a frame whose write ends after the
//!                   next frame is due would be a lost instant
//!
//! Processor time: this thread (cycle counter), this process (GetProcessTimes)
//! and, where Windows allows reading it, the System process that does the
//! lazy writes of the file cache.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_MS, HNS_PER_SECOND};
use crate::scratch::ScratchFolder;
use crate::stats::Summary;

const FRAME_BYTES: usize = 1920 * 1080 * 4;
/// Sector alignment for unbuffered writes; FRAME_BYTES is 2,025 such sectors.
const ALIGN: usize = 4096;
const SYSTEM_PROCESS_ID: u32 = 4;

#[derive(Clone, Copy)]
enum Mode {
    Buffered,
    WriteThrough,
}

struct PhasePlan {
    name: &'static str,
    mode: Mode,
    frames: usize,
    pace_hns: Option<i64>,
}

const PHASES: [PhasePlan; 3] = [
    PhasePlan {
        name: "buffered",
        mode: Mode::Buffered,
        frames: 60,
        pace_hns: None,
    },
    PhasePlan {
        name: "write_through",
        mode: Mode::WriteThrough,
        frames: 60,
        pace_hns: None,
    },
    PhasePlan {
        name: "paced_20fps_buffered",
        mode: Mode::Buffered,
        frames: 40,
        pace_hns: Some(50 * HNS_PER_MS),
    },
];

pub fn run() -> Result<()> {
    let rate = CycleRate::calibrate();
    let mut folder = ScratchFolder::create("disk")?;
    println!(
        "M5 folder={} free_bytes_before={}",
        folder.path().display(),
        folder.free_bytes()?
    );
    // An aligned buffer of pseudo-random bytes (nothing a disk could compress away).
    let mut storage = vec![0_u8; FRAME_BYTES + ALIGN];
    let offset = storage.as_ptr().align_offset(ALIGN);
    let frame = &mut storage[offset..offset + FRAME_BYTES];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for chunk in frame.chunks_exact_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    for plan in &PHASES {
        let outcome = write_phase(&mut folder, frame, plan, rate);
        folder.clear()?;
        outcome?;
    }
    println!(
        "M5 total_bytes_written={} free_bytes_after={}",
        folder.written(),
        folder.free_bytes()?
    );
    Ok(())
}

fn write_phase(
    folder: &mut ScratchFolder,
    frame: &[u8],
    plan: &PhasePlan,
    rate: CycleRate,
) -> Result<()> {
    folder.reserve((plan.frames * FRAME_BYTES) as u64)?;
    let system = SystemProcessTime::open();
    let system_start = system.read();
    let (wall_start, cycles_start, process_start) = (
        clock::now_hns(),
        clock::thread_cycles(),
        clock::current_process_time_hns(),
    );
    let mut write_ms = Vec::with_capacity(plan.frames);
    let mut late = 0;
    for index in 0..plan.frames {
        if let Some(pace) = plan.pace_hns {
            clock::sleep_until(wall_start + index as i64 * pace);
        }
        let path = folder.next_file(FRAME_BYTES as u64)?;
        let begin = clock::now_hns();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        if let Mode::WriteThrough = plan.mode {
            options.custom_flags(FILE_FLAG_NO_BUFFERING.0 | FILE_FLAG_WRITE_THROUGH.0);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.write_all(frame)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        drop(file);
        let end = clock::now_hns();
        write_ms.push(clock::hns_to_ms(end - begin));
        if let Some(pace) = plan.pace_hns
            && end > wall_start + (index as i64 + 1) * pace
        {
            late += 1;
        }
    }
    let wall_s = (clock::now_hns() - wall_start) as f64 / HNS_PER_SECOND as f64;
    let thread_s = rate.seconds(clock::thread_cycles() - cycles_start);
    let process_s =
        (clock::current_process_time_hns() - process_start) as f64 / HNS_PER_SECOND as f64;
    let bytes = (plan.frames * FRAME_BYTES) as f64;
    let pixels = bytes / 4.0;
    let system_s = match (system_start, system.read()) {
        (Some(start), Some(end)) => format!("{:.3}", (end - start) as f64 / HNS_PER_SECOND as f64),
        _ => "unavailable".to_owned(),
    };
    println!(
        "M5 phase={} frames={} bytes={bytes:.0} wall_s={wall_s:.3} mb_per_s={:.1} write_ms {} late_frames={late}",
        plan.name,
        plan.frames,
        bytes / 1e6 / wall_s,
        Summary::of(&write_ms)
    );
    println!(
        "M5 phase={} thread_cpu_s={thread_s:.3} process_cpu_s={process_s:.3} system_process_cpu_s={system_s} thread_cpu_ns_per_px={:.4} thread_cpu_ms_per_frame={:.3} share_of_one_thread={:.3}",
        plan.name,
        thread_s * 1e9 / pixels,
        thread_s * 1e3 / plan.frames as f64,
        thread_s / wall_s
    );
    Ok(())
}

/// Processor time of the System process (PID 4), which does the cache's lazy
/// writes; None where Windows refuses the query.
struct SystemProcessTime {
    handle: Option<windows::Win32::Foundation::HANDLE>,
}

impl SystemProcessTime {
    fn open() -> Self {
        // SAFETY: a query-only handle, closed on drop.
        Self {
            handle: unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, SYSTEM_PROCESS_ID)
            }
            .ok(),
        }
    }

    fn read(&self) -> Option<i64> {
        self.handle.and_then(clock::process_time_hns)
    }
}

impl Drop for SystemProcessTime {
    fn drop(&mut self) {
        if let Some(handle) = self.handle {
            // SAFETY: the handle opened above.
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}
