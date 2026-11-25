// Single-thread cost per pixel of the recorder's processor work, measured on this machine.
// Build: rustc -O bench.rs -o bench.exe      Run: bench.exe
// convert_scalar is the same code shape as ClipCanvas::convert in server/capture_clip.rs (commit be2447f8).
use std::hint::black_box;
use std::time::Instant;

fn clamp_u8(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

fn convert_scalar(bgra: &[u8], stride_px: usize, width: usize, height: usize, nv12: &mut [u8]) {
    let rgb_at = |col: usize, row: usize| -> (i32, i32, i32) {
        let offset = (row * stride_px + col) * 4;
        bgra.get(offset..offset + 3)
            .map_or((0, 0, 0), |px| (i32::from(px[2]), i32::from(px[1]), i32::from(px[0])))
    };
    let (luma, chroma) = nv12.split_at_mut(width * height);
    for row in 0..height {
        for col in 0..width {
            let (red, green, blue) = rgb_at(col, row);
            luma[row * width + col] = clamp_u8(((47 * red + 157 * green + 16 * blue + 128) >> 8) + 16);
        }
    }
    for row in (0..height).step_by(2) {
        for col in (0..width).step_by(2) {
            let mut sum = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (red, green, blue) = rgb_at(col + dx, row + dy);
                sum = (sum.0 + red, sum.1 + green, sum.2 + blue);
            }
            let (red, green, blue) = (sum.0 / 4, sum.1 / 4, sum.2 / 4);
            let at = (row / 2) * width + col;
            chroma[at] = clamp_u8(((-26 * red - 87 * green + 112 * blue + 128) >> 8) + 128);
            chroma[at + 1] = clamp_u8(((112 * red - 102 * green - 10 * blue + 128) >> 8) + 128);
        }
    }
}

/// 8x8 block change test with the viewer's rule: a pixel differs when any of R, G, B differs by more than 32.
fn changed_blocks(a: &[u8], b: &[u8], width: usize, height: usize) -> usize {
    let mut blocks = 0;
    for by in (0..height).step_by(8) {
        for bx in (0..width).step_by(8) {
            let mut pixels = 0u32;
            for y in by..(by + 8).min(height) {
                let row = y * width * 4;
                for x in bx..(bx + 8).min(width) {
                    let i = row + x * 4;
                    let d = a[i].abs_diff(b[i]).max(a[i + 1].abs_diff(b[i + 1])).max(a[i + 2].abs_diff(b[i + 2]));
                    if d > 32 {
                        pixels += 1;
                    }
                }
            }
            if pixels >= 3 {
                blocks += 1;
            }
        }
    }
    blocks
}

fn main() {
    let (w, h) = (1920usize, 1080usize);
    let px = (w * h) as f64;
    let src: Vec<u8> = (0..w * h * 4).map(|i| ((i * 31 + i / 7) % 251) as u8).collect();
    let other: Vec<u8> = src.iter().map(|v| v.wrapping_add(3)).collect();
    let mut dst = vec![0u8; w * h * 4];
    let mut nv12 = vec![0u8; w * h * 3 / 2];
    let rounds = 40;

    let t = Instant::now();
    for _ in 0..rounds {
        dst.copy_from_slice(black_box(&src));
        black_box(&dst);
    }
    let copy = t.elapsed().as_secs_f64() / rounds as f64;

    let t = Instant::now();
    for _ in 0..rounds {
        convert_scalar(black_box(&src), w, w, h, &mut nv12);
        black_box(&nv12);
    }
    let conv = t.elapsed().as_secs_f64() / rounds as f64;

    let t = Instant::now();
    let mut blocks = 0;
    for _ in 0..rounds {
        blocks += changed_blocks(black_box(&src), black_box(&other), w, h);
    }
    let diff = t.elapsed().as_secs_f64() / rounds as f64;

    println!("frame 1920x1080, single thread, {rounds} rounds, blocks={blocks}");
    println!("copy_bgra    ms_per_frame={:.3} ns_per_px={:.3}", copy * 1e3, copy * 1e9 / px);
    println!("convert_nv12 ms_per_frame={:.3} ns_per_px={:.3}", conv * 1e3, conv * 1e9 / px);
    println!("block_diff   ms_per_frame={:.3} ns_per_px={:.3}", diff * 1e3, diff * 1e9 / px);
}
