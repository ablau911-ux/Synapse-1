//! Experiments steps 2 and 3, way Z (docs/clip-recorder-experiments-plan.md,
//! "Procedure" 2 and 3; ways ZW0 to ZW19 of docs/clip-recorder-ways.md): one
//! zstd way compresses one corpus folder, then the clip is decoded and every
//! pixel is compared with the corpus.
//!
//! `clip-probe exp-z folder=<corpus folder> way=ZW1 level=-3 out=<scratch>
//!  [layout=bgr|bgra|planar|sg|sgp|ycocg|predl|predu] [mode=raw|whole|xor|sub|
//!  tiles|rect|box|prefix|stream|switch] [tile=32] [gap=0] [wlog=23] [ldm=0|1]
//!  [dict=0|110|1024] [workers=0] [kz=20|all] [theta=25] [rep=1] [keep=0]`
//!
//! Measurement rules M1 to M4 and M8 of the plan (shared code in `exp_measure`):
//! the corpus is streamed one instant at a time. Each new frame is decompressed
//! (timed apart, `corpus_*`) and expanded to B,G,R,A, the capture format, before
//! the timer (M1). The way's CPU (`way_*`, M2) is the cycle counter around all of
//! its work: the conversion to its layout (`convert_*`), context creation and
//! compression (`comp_*`), writing the frame files, `frames.jsonl` and `clip.json`
//! (`write_*`) and the dictionary training (`train_*`). M_0 is taken after the
//! input is prepared; RAM is the OS peak of private bytes minus M_0 (M3). Bytes
//! count every retained file (M4). ldm is set on or off explicitly; ZW9 codes the
//! merged changed-tile rectangles, ZW9b one global box (M8). The run prints one
//! JSON line and deletes its scratch clip unless `keep=1`.

use std::ffi::c_void;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use zstd::zstd_safe::zstd_sys as sys;

use crate::Result;
use crate::exp_cpu::Spent;
use crate::exp_measure::{CaptureFrame, ClipFiles, RamMeter, bgra_to_bgr};
use crate::exp_meter::{Args, MemorySampler, memory_now, os_peak_working_set, wall_ns};

/// Dictionary training uses the first 20 new frames (plan factor Z4).
const DICT_TRAIN_FRAMES: usize = 20;

fn zstd_check(code: usize, step: &str) -> Result<usize> {
    // SAFETY: pure functions on an error code.
    if unsafe { sys::ZSTD_isError(code) } != 0 {
        let name = unsafe { std::ffi::CStr::from_ptr(sys::ZSTD_getErrorName(code)) };
        return Err(format!("{step}: {}", name.to_string_lossy()));
    }
    Ok(code)
}

/// The byte order of a frame before zstd sees it. Every layout is reversible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    Bgr,
    Bgra,
    Planar,
    SubGreen,
    SubGreenPlanar,
    YCoCg,
    PredLeft,
    PredUp,
}

impl Layout {
    fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "bgr" => Self::Bgr,
            "bgra" => Self::Bgra,
            "planar" => Self::Planar,
            "sg" => Self::SubGreen,
            "sgp" => Self::SubGreenPlanar,
            "ycocg" => Self::YCoCg,
            "predl" => Self::PredLeft,
            "predu" => Self::PredUp,
            _ => return Err(format!("unknown layout {text}")),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bgr => "bgr",
            Self::Bgra => "bgra",
            Self::Planar => "planar",
            Self::SubGreen => "sg",
            Self::SubGreenPlanar => "sgp",
            Self::YCoCg => "ycocg",
            Self::PredLeft => "predl",
            Self::PredUp => "predu",
        }
    }

    const fn bytes_per_pixel(self) -> usize {
        if matches!(self, Self::Bgra) { 4 } else { 3 }
    }

    const fn planar(self) -> bool {
        matches!(self, Self::Planar | Self::SubGreenPlanar | Self::YCoCg)
    }

    /// The frame as an image of rows × row bytes, and bytes per pixel in a row.
    fn grid(self, w: usize, h: usize) -> (usize, usize, usize) {
        if self.planar() { (3 * h, w, 1) } else { (h, w * self.bytes_per_pixel(), self.bytes_per_pixel()) }
    }

    /// The capture's B,G,R,A to this layout (M1: the way's own, timed conversion).
    fn forward(self, bgra: &[u8], w: usize, out: &mut Vec<u8>) {
        let px = bgra.len() / 4;
        match self {
            Self::Bgr => bgra_to_bgr(bgra, out),
            Self::Bgra => {
                out.clear();
                out.extend_from_slice(bgra);
            }
            Self::Planar | Self::SubGreenPlanar | Self::YCoCg => {
                out.resize(px * 3, 0);
                let (b, rest) = out.split_at_mut(px);
                let (g, r) = rest.split_at_mut(px);
                for (k, p) in bgra.chunks_exact(4).enumerate() {
                    let (pb, pg, pr) = Self::color_forward(self, p[0], p[1], p[2]);
                    (b[k], g[k], r[k]) = (pb, pg, pr);
                }
            }
            Self::SubGreen => {
                out.resize(px * 3, 0);
                for (o, p) in out.chunks_exact_mut(3).zip(bgra.chunks_exact(4)) {
                    o.copy_from_slice(&[p[0].wrapping_sub(p[1]), p[1], p[2].wrapping_sub(p[1])]);
                }
            }
            Self::PredLeft => {
                // B,G,R first, then each byte minus its left neighbour, right to left so the neighbour is still original.
                bgra_to_bgr(bgra, out);
                let row = w * 3;
                for line in out.chunks_exact_mut(row) {
                    for x in (3..row).rev() {
                        line[x] = line[x].wrapping_sub(line[x - 3]);
                    }
                }
            }
            Self::PredUp => {
                bgra_to_bgr(bgra, out);
                let row = w * 3;
                for x in (row..out.len()).rev() {
                    out[x] = out[x].wrapping_sub(out[x - row]);
                }
            }
        }
    }

    /// One pixel's three planes: plain, subtract green, or lossless YCoCg-R modulo 256.
    fn color_forward(self, b: u8, g: u8, r: u8) -> (u8, u8, u8) {
        match self {
            Self::SubGreenPlanar => (b.wrapping_sub(g), g, r.wrapping_sub(g)),
            Self::YCoCg => {
                let co = r.wrapping_sub(b);
                let t = b.wrapping_add(co >> 1);
                let cg = g.wrapping_sub(t);
                let y = t.wrapping_add(cg >> 1);
                (y, co, cg)
            }
            _ => (b, g, r),
        }
    }

    fn color_inverse(self, p0: u8, p1: u8, p2: u8) -> (u8, u8, u8) {
        match self {
            Self::SubGreenPlanar => (p0.wrapping_add(p1), p1, p2.wrapping_add(p1)),
            Self::YCoCg => {
                let (y, co, cg) = (p0, p1, p2);
                let t = y.wrapping_sub(cg >> 1);
                let g = cg.wrapping_add(t);
                let b = t.wrapping_sub(co >> 1);
                let r = b.wrapping_add(co);
                (b, g, r)
            }
            _ => (p0, p1, p2),
        }
    }

    /// This layout back to B,G,R interleaved.
    fn inverse(self, data: &[u8], w: usize, bgr: &mut [u8]) {
        let px = bgr.len() / 3;
        match self {
            Self::Bgr => bgr.copy_from_slice(data),
            Self::Bgra => {
                for (o, i) in bgr.chunks_exact_mut(3).zip(data.chunks_exact(4)) {
                    o.copy_from_slice(&i[..3]);
                }
            }
            Self::Planar | Self::SubGreenPlanar | Self::YCoCg => {
                let (b, rest) = data.split_at(px);
                let (g, r) = rest.split_at(px);
                for (k, o) in bgr.chunks_exact_mut(3).enumerate() {
                    let (pb, pg, pr) = self.color_inverse(b[k], g[k], r[k]);
                    o.copy_from_slice(&[pb, pg, pr]);
                }
            }
            Self::SubGreen => {
                for (o, p) in bgr.chunks_exact_mut(3).zip(data.chunks_exact(3)) {
                    o.copy_from_slice(&[p[0].wrapping_add(p[1]), p[1], p[2].wrapping_add(p[1])]);
                }
            }
            Self::PredLeft => {
                let row = w * 3;
                for (o, i) in bgr.chunks_exact_mut(row).zip(data.chunks_exact(row)) {
                    o[..3].copy_from_slice(&i[..3]);
                    for x in 3..row {
                        o[x] = i[x].wrapping_add(o[x - 3]);
                    }
                }
            }
            Self::PredUp => {
                let row = w * 3;
                bgr[..row].copy_from_slice(&data[..row]);
                for x in row..data.len() {
                    bgr[x] = data[x].wrapping_add(bgr[x - row]);
                }
            }
        }
    }
}

/// How consecutive frames are coded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Raw,
    Whole,
    Xor,
    Sub,
    Tiles,
    Rect,
    Box,
    Prefix,
    Stream,
    Switch,
}

impl Mode {
    fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "raw" => Self::Raw,
            "whole" => Self::Whole,
            "xor" => Self::Xor,
            "sub" => Self::Sub,
            "tiles" => Self::Tiles,
            "rect" => Self::Rect,
            "box" => Self::Box,
            "prefix" => Self::Prefix,
            "stream" => Self::Stream,
            "switch" => Self::Switch,
            _ => return Err(format!("unknown mode {text}")),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Whole => "whole",
            Self::Xor => "xor",
            Self::Sub => "sub",
            Self::Tiles => "tiles",
            Self::Rect => "rect",
            Self::Box => "box",
            Self::Prefix => "prefix",
            Self::Stream => "stream",
            Self::Switch => "switch",
        }
    }

    /// The mode keeps the previous frame (ν = 1 of the ways register).
    const fn keeps_previous(self) -> bool {
        !matches!(self, Self::Raw | Self::Whole)
    }
}

/// One way with every option of this run (ways register, section 3).
#[derive(Clone, Debug)]
struct Way {
    id: String,
    layout: Layout,
    mode: Mode,
    level: i32,
    tile: usize,
    /// ZW9: tiles at most this many unchanged tiles apart join one rectangle.
    gap: usize,
    window_log: u32,
    ldm: bool,
    dict_kb: usize,
    workers: u32,
    kz: usize,
    theta: f64,
    /// Output window of `ZSTD_compressStream2` per call; 0 = one-shot (the product's `bulk::compress`).
    drain: usize,
}

impl Way {
    /// The way's own layout and mode (ZW19 takes both from the arguments).
    fn defaults(id: &str) -> Result<(&'static str, &'static str)> {
        Ok(match id {
            "ZW0" => ("bgr", "raw"),
            "ZW1" | "ZW16" | "ZW17" => ("bgr", "whole"),
            "ZW2" => ("bgra", "whole"),
            "ZW3" => ("planar", "whole"),
            "ZW4" => ("sg", "whole"),
            "ZW5" => ("ycocg", "whole"),
            "ZW6" => ("predl", "whole"),
            "ZW7" => ("bgr", "xor"),
            "ZW8" => ("bgr", "tiles"),
            "ZW9" => ("bgr", "rect"),
            "ZW9b" => ("bgr", "box"),
            "ZW12" | "ZW15" => ("bgr", "prefix"),
            "ZW13" | "ZW14" => ("bgr", "stream"),
            "ZW18" => ("bgr", "switch"),
            "ZW19" => ("planar", "xor"),
            "ZW10" | "ZW11" | "ZW20" => {
                return Err(format!("{id} is not run: see docs/clip-recorder-ways.md section 4"));
            }
            _ => return Err(format!("unknown way {id}")),
        })
    }

    fn from_args(args: &Args, frame_bytes: usize) -> Result<Self> {
        let id = args.text("way")?.to_owned();
        let (layout, mode) = Self::defaults(&id)?;
        let mode = Mode::parse(args.text_or("mode", mode))?;
        // Prefix and stream must reach one frame back: W ≥ ⌈log2(2 · px · 3)⌉ (ways register ZW12, ZW13).
        let reach = (2 * frame_bytes).next_power_of_two().trailing_zeros();
        let kz = match args.text_or("kz", "20") {
            "all" => usize::MAX,
            text => text.parse().map_err(|_| format!("kz={text} is not a number"))?,
        };
        Ok(Self {
            layout: Layout::parse(args.text_or("layout", layout))?,
            mode,
            level: args.number("level", -3)?,
            tile: args.number("tile", 32)?,
            gap: args.number("gap", 0)?,
            window_log: args.number("wlog", if matches!(mode, Mode::Prefix | Mode::Stream) { reach } else { 0 })?,
            ldm: args.flag("ldm", id == "ZW14" || id == "ZW15"),
            dict_kb: args.number("dict", if id == "ZW16" { 110 } else { 0 })?,
            workers: args.number("workers", 0)?,
            kz: kz.max(1),
            theta: args.number("theta", 25.0)?,
            drain: args.number("drain", 0)?,
            id,
        })
    }

    fn is_key(&self, k: usize) -> bool {
        !self.mode.keeps_previous() || k % self.kz == 0
    }

    /// The last frame of a keyframe group: the stream mode ends its zstd frame here.
    fn ends_group(&self, k: usize) -> bool {
        self.kz != usize::MAX && (k + 1) % self.kz == 0
    }

    /// Long-distance matching set explicitly, the same on the context and in the
    /// estimate: zstd's estimate does not fill unset ldm fields (minMatch 0 divides by zero).
    /// hashLog = W − 7, minMatch 64, bucketSizeLog 4, hashRateLog 7 ([I], ways register ZW14).
    fn ldm_params(&self) -> [(sys::ZSTD_cParameter, u32); 4] {
        [
            (sys::ZSTD_cParameter::ZSTD_c_ldmHashLog, self.window_log.saturating_sub(7).max(6)),
            (sys::ZSTD_cParameter::ZSTD_c_ldmMinMatch, 64),
            (sys::ZSTD_cParameter::ZSTD_c_ldmBucketSizeLog, 4),
            (sys::ZSTD_cParameter::ZSTD_c_ldmHashRateLog, 7),
        ]
    }

    /// Frame-sized buffers the way holds while compressing (B_c) and decoding (B_d),
    /// in units of px · 3 bytes (a B,G,R,A layout buffer is 4/3). Every buffer is
    /// allocated up front, so the count is known before the run:
    /// compress: output (+ layout copy) (+ previous frame) (+ gathered tiles);
    /// decode: frame (+ delta or prefix target when K_z > 1, always for the switch) (+ B,G,R copy).
    /// The compressed frame the viewer reads is `b_max` bytes, counted apart.
    fn buffers(&self) -> (f64, f64) {
        let q = self.layout.bytes_per_pixel() as f64 / 3.0;
        // Compress: the layout copy of the B,G,R,A capture frame (M1); the B,G,R,A layout reads the frame as captured.
        let view = if self.layout == Layout::Bgra { 0.0 } else { q };
        // Decode: the B,G,R copy compared with the corpus; none for the B,G,R layout.
        let bgr = if self.layout == Layout::Bgr { 0.0 } else { 1.0 };
        let delta_target = if self.kz > 1 { q } else { 0.0 };
        match self.mode {
            Mode::Raw | Mode::Whole => (q + view, q + bgr),
            Mode::Xor | Mode::Sub | Mode::Prefix => (2.0 * q + view, q + delta_target + bgr),
            Mode::Tiles | Mode::Rect | Mode::Box => (3.0 * q + view, q + delta_target + bgr),
            Mode::Switch => (3.0 * q + view, 2.0 * q + bgr),
            Mode::Stream => (q + view, q + bgr),
        }
    }

    fn json(&self) -> String {
        let kz = if self.kz == usize::MAX { "\"all\"".to_owned() } else { self.kz.to_string() };
        format!(
            "\"way\":\"{}\",\"layout\":\"{}\",\"mode\":\"{}\",\"level\":{},\"tile\":{},\"gap\":{},\"wlog\":{},\"ldm\":{},\"dict\":{},\"workers\":{},\"kz\":{kz},\"theta\":{},\"drain\":{}",
            self.id,
            self.layout.name(),
            self.mode.name(),
            self.level,
            self.tile,
            self.gap,
            self.window_log,
            u8::from(self.ldm),
            self.dict_kb,
            self.workers,
            self.theta,
            self.drain
        )
    }
}

/// zstd's own estimates of the contexts (equation 4: M_cctx, M_dctx).
struct Estimate;

impl Estimate {
    /// `ZSTD_estimateCCtxSize_usingCCtxParams` (one stream: `ZSTD_estimateCStreamSize_usingCCtxParams`)
    /// with the parameters `ZSTD_getCParams` picks for this level and source size, plus the CDict.
    fn cctx(way: &Way, src: usize, dict: usize) -> usize {
        // SAFETY: plain calls on a parameter object this function owns.
        unsafe {
            let cp = sys::ZSTD_getCParams(way.level, src as u64, dict);
            let params = sys::ZSTD_createCCtxParams();
            let window = if way.window_log > 0 { way.window_log } else { cp.windowLog };
            let set = |p, v: u32| sys::ZSTD_CCtxParams_setParameter(params, p, v as i32);
            set(sys::ZSTD_cParameter::ZSTD_c_windowLog, window);
            set(sys::ZSTD_cParameter::ZSTD_c_chainLog, cp.chainLog);
            set(sys::ZSTD_cParameter::ZSTD_c_hashLog, cp.hashLog);
            set(sys::ZSTD_cParameter::ZSTD_c_searchLog, cp.searchLog);
            set(sys::ZSTD_cParameter::ZSTD_c_minMatch, cp.minMatch);
            set(sys::ZSTD_cParameter::ZSTD_c_targetLength, cp.targetLength);
            set(sys::ZSTD_cParameter::ZSTD_c_strategy, cp.strategy as u32);
            set(sys::ZSTD_cParameter::ZSTD_c_enableLongDistanceMatching, if way.ldm { 1 } else { 2 });
            if way.ldm {
                for (param, value) in way.ldm_params() {
                    set(param, value);
                }
            }
            // The streaming call (one stream, or the corpus writer's drained call in buffered mode) adds its in and out buffers.
            let size = if way.mode == Mode::Stream || way.drain > 0 {
                sys::ZSTD_estimateCStreamSize_usingCCtxParams(params)
            } else {
                sys::ZSTD_estimateCCtxSize_usingCCtxParams(params)
            };
            sys::ZSTD_freeCCtxParams(params);
            let cdict = if dict > 0 { sys::ZSTD_estimateCDictSize(dict, way.level) } else { 0 };
            if sys::ZSTD_isError(size) != 0 { 0 } else { size + cdict }
        }
    }

    /// `ZSTD_estimateDCtxSize` (one stream: `ZSTD_estimateDStreamSize(2^W)`), plus the DDict.
    fn dctx(way: &Way, dict: usize) -> usize {
        // SAFETY: pure size functions.
        unsafe {
            let base = if way.mode == Mode::Stream {
                sys::ZSTD_estimateDStreamSize(1usize << way.window_log)
            } else {
                sys::ZSTD_estimateDCtxSize()
            };
            let ddict = if dict > 0 {
                sys::ZSTD_estimateDDictSize(dict, sys::ZSTD_dictLoadMethod_e::ZSTD_dlm_byCopy)
            } else {
                0
            };
            base + ddict
        }
    }
}

/// One zstd compression context with this way's parameters.
struct Packer {
    cctx: *mut sys::ZSTD_CCtx,
    /// 0: one-shot `ZSTD_compress2`; else the output window of the corpus writer's call.
    drain: usize,
}

impl Packer {
    fn new(way: &Way, dict: &[u8]) -> Result<Self> {
        // SAFETY: a new context, freed in Drop.
        let packer = Self { cctx: unsafe { sys::ZSTD_createCCtx() }, drain: way.drain };
        if packer.cctx.is_null() {
            return Err("ZSTD_createCCtx returned null".into());
        }
        packer.set(sys::ZSTD_cParameter::ZSTD_c_compressionLevel, way.level)?;
        if way.window_log > 0 {
            packer.set(sys::ZSTD_cParameter::ZSTD_c_windowLog, way.window_log as i32)?;
        }
        // M8: ldm always set, 1 enabled or 2 disabled; unset (0) lets zstd enable it itself at W >= 27 and btopt or slower.
        packer.set(sys::ZSTD_cParameter::ZSTD_c_enableLongDistanceMatching, if way.ldm { 1 } else { 2 })?;
        if way.ldm {
            for (param, value) in way.ldm_params() {
                packer.set(param, value as i32)?;
            }
        }
        if way.workers > 0 {
            packer.set(sys::ZSTD_cParameter::ZSTD_c_nbWorkers, way.workers as i32)?;
        }
        if !dict.is_empty() {
            // SAFETY: the dictionary is copied by zstd.
            zstd_check(
                unsafe { sys::ZSTD_CCtx_loadDictionary(packer.cctx, dict.as_ptr().cast(), dict.len()) },
                "ZSTD_CCtx_loadDictionary",
            )?;
        }
        Ok(packer)
    }

    fn set(&self, param: sys::ZSTD_cParameter, value: i32) -> Result<()> {
        // SAFETY: a live context.
        zstd_check(unsafe { sys::ZSTD_CCtx_setParameter(self.cctx, param, value) }, "ZSTD_CCtx_setParameter")
            .map(|_| ())
    }

    /// One independent zstd frame of `src` (with the previous frame as prefix when given), into `out`.
    fn frame(&mut self, src: &[u8], prefix: Option<&[u8]>, out: &mut Vec<u8>) -> Result<()> {
        // SAFETY: `out` has the capacity passed; the prefix outlives this one call, which is all zstd keeps it for.
        unsafe {
            if let Some(prefix) = prefix {
                zstd_check(sys::ZSTD_CCtx_refPrefix(self.cctx, prefix.as_ptr().cast(), prefix.len()), "refPrefix")?;
            }
            // Slack for the tile bitmap or rectangle header put in front later: no reallocation.
            let bound = sys::ZSTD_compressBound(src.len()) + 65536;
            out.clear();
            out.reserve(bound);
            if self.drain > 0 && prefix.is_none() {
                return self.drained(src, out);
            }
            let n = zstd_check(
                sys::ZSTD_compress2(self.cctx, out.as_mut_ptr().cast(), out.capacity(), src.as_ptr().cast(), src.len()),
                "ZSTD_compress2",
            )?;
            out.set_len(n);
        }
        Ok(())
    }

    /// The corpus writer's call (`corpus/render.mjs`, Node `zlib.zstdCompressSync`):
    /// `ZSTD_compressStream2(ZSTD_e_end)` with the output drained `drain` bytes at a
    /// time. An output window below the bound puts zstd in buffered mode, whose
    /// blocks differ from one-shot `ZSTD_compress2` (same level, same frame).
    fn drained(&mut self, src: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let mut input = sys::ZSTD_inBuffer { src: src.as_ptr().cast(), size: src.len(), pos: 0 };
        loop {
            if out.capacity() - out.len() < self.drain {
                out.reserve(self.drain);
            }
            // SAFETY: zstd writes at most `size` bytes into the spare capacity.
            unsafe {
                let mut output = sys::ZSTD_outBuffer {
                    dst: out.as_mut_ptr().add(out.len()).cast::<c_void>(),
                    size: self.drain,
                    pos: 0,
                };
                let left = zstd_check(
                    sys::ZSTD_compressStream2(self.cctx, &raw mut output, &raw mut input, sys::ZSTD_EndDirective::ZSTD_e_end),
                    "ZSTD_compressStream2",
                )?;
                out.set_len(out.len() + output.pos);
                if left == 0 {
                    return Ok(());
                }
            }
        }
    }

    /// One frame into the running stream, flushed (`ZSTD_e_flush`) or ending the zstd frame (`ZSTD_e_end`).
    fn chunk(&mut self, src: &[u8], end: bool, out: &mut Vec<u8>) -> Result<()> {
        out.clear();
        // SAFETY: zstd writes into the spare capacity, which grows before each call.
        unsafe {
            out.reserve(sys::ZSTD_compressBound(src.len()) + 1024);
            let mut input = sys::ZSTD_inBuffer { src: src.as_ptr().cast(), size: src.len(), pos: 0 };
            let directive = if end { sys::ZSTD_EndDirective::ZSTD_e_end } else { sys::ZSTD_EndDirective::ZSTD_e_flush };
            loop {
                if out.capacity() - out.len() < 1024 {
                    out.reserve(64 * 1024);
                }
                let mut output = sys::ZSTD_outBuffer {
                    dst: out.as_mut_ptr().add(out.len()).cast::<c_void>(),
                    size: out.capacity() - out.len(),
                    pos: 0,
                };
                let left = zstd_check(
                    sys::ZSTD_compressStream2(self.cctx, &raw mut output, &raw mut input, directive),
                    "ZSTD_compressStream2",
                )?;
                out.set_len(out.len() + output.pos);
                if left == 0 && input.pos == input.size {
                    return Ok(());
                }
            }
        }
    }

    fn size(&self) -> usize {
        // SAFETY: a live context.
        unsafe { sys::ZSTD_sizeof_CCtx(self.cctx) }
    }

    /// A parameter as the context holds it (the read-back of M5 and M8).
    fn get(&self, param: sys::ZSTD_cParameter) -> i32 {
        let mut value = 0;
        // SAFETY: a live context and an out-parameter.
        let code = unsafe { sys::ZSTD_CCtx_getParameter(self.cctx, param, &raw mut value) };
        // SAFETY: pure function on an error code.
        if unsafe { sys::ZSTD_isError(code) } != 0 { -1 } else { value }
    }
}

impl Drop for Packer {
    fn drop(&mut self) {
        // SAFETY: created in `new`, freed once.
        unsafe { sys::ZSTD_freeCCtx(self.cctx) };
    }
}

/// One zstd decompression context.
struct Unpacker {
    dctx: *mut sys::ZSTD_DCtx,
}

impl Unpacker {
    fn new(dict: &[u8]) -> Result<Self> {
        // SAFETY: a new context, freed in Drop.
        let unpacker = Self { dctx: unsafe { sys::ZSTD_createDCtx() } };
        if unpacker.dctx.is_null() {
            return Err("ZSTD_createDCtx returned null".into());
        }
        if !dict.is_empty() {
            // SAFETY: the dictionary is copied by zstd.
            zstd_check(
                unsafe { sys::ZSTD_DCtx_loadDictionary(unpacker.dctx, dict.as_ptr().cast(), dict.len()) },
                "ZSTD_DCtx_loadDictionary",
            )?;
        }
        Ok(unpacker)
    }

    /// One zstd frame into `out`; returns the bytes written.
    fn frame(&mut self, src: &[u8], prefix: Option<&[u8]>, out: &mut [u8]) -> Result<usize> {
        // SAFETY: sizes passed with their buffers; the prefix outlives the call.
        unsafe {
            if let Some(prefix) = prefix {
                zstd_check(sys::ZSTD_DCtx_refPrefix(self.dctx, prefix.as_ptr().cast(), prefix.len()), "DCtx refPrefix")?;
            }
            zstd_check(
                sys::ZSTD_decompressDCtx(self.dctx, out.as_mut_ptr().cast(), out.len(), src.as_ptr().cast(), src.len()),
                "ZSTD_decompressDCtx",
            )
        }
    }

    /// One chunk of the stream; returns the bytes written.
    fn chunk(&mut self, src: &[u8], out: &mut [u8]) -> Result<usize> {
        let mut input = sys::ZSTD_inBuffer { src: src.as_ptr().cast(), size: src.len(), pos: 0 };
        let mut output = sys::ZSTD_outBuffer { dst: out.as_mut_ptr().cast(), size: out.len(), pos: 0 };
        loop {
            let before = (input.pos, output.pos);
            // SAFETY: both buffers live for the call.
            zstd_check(
                unsafe { sys::ZSTD_decompressStream(self.dctx, &raw mut output, &raw mut input) },
                "ZSTD_decompressStream",
            )?;
            let stalled = before == (input.pos, output.pos);
            if input.pos == input.size && (output.pos == output.size || stalled) {
                return Ok(output.pos);
            }
            if stalled {
                return Err(format!("ZSTD_decompressStream made no progress at {} of {} bytes", input.pos, input.size));
            }
        }
    }
}

impl Drop for Unpacker {
    fn drop(&mut self) {
        // SAFETY: created in `new`, freed once.
        unsafe { sys::ZSTD_freeDCtx(self.dctx) };
    }
}

/// Which tiles of a rows × row-bytes image differ between two frames.
struct TileGrid {
    rows: usize,
    row_bytes: usize,
    tile_rows: usize,
    tile_bytes: usize,
    across: usize,
    down: usize,
}

impl TileGrid {
    fn new(layout: Layout, w: usize, h: usize, tile: usize) -> Self {
        let (rows, row_bytes, bpp) = layout.grid(w, h);
        let tile_bytes = tile * bpp;
        Self {
            rows,
            row_bytes,
            tile_rows: tile,
            tile_bytes,
            across: row_bytes.div_ceil(tile_bytes),
            down: rows.div_ceil(tile),
        }
    }

    fn count(&self) -> usize {
        self.across * self.down
    }

    /// Tile `t` as (first row, last row + 1, first byte, last byte + 1).
    fn span(&self, t: usize) -> (usize, usize, usize, usize) {
        let (ty, tx) = (t / self.across, t % self.across);
        let (r0, c0) = (ty * self.tile_rows, tx * self.tile_bytes);
        (r0, (r0 + self.tile_rows).min(self.rows), c0, (c0 + self.tile_bytes).min(self.row_bytes))
    }

    /// One bit per tile, set when the tile changed; returns the changed count.
    fn compare(&self, cur: &[u8], prev: &[u8], bits: &mut Vec<u8>) -> usize {
        bits.clear();
        bits.resize(self.count().div_ceil(8), 0);
        let mut changed = 0;
        for t in 0..self.count() {
            let (r0, r1, c0, c1) = self.span(t);
            let differs = (r0..r1).any(|r| {
                let at = r * self.row_bytes;
                cur[at + c0..at + c1] != prev[at + c0..at + c1]
            });
            if differs {
                bits[t / 8] |= 1 << (t % 8);
                changed += 1;
            }
        }
        changed
    }

    /// The changed tiles' rows, in tile order.
    fn gather(&self, cur: &[u8], bits: &[u8], out: &mut Vec<u8>) {
        out.clear();
        for t in (0..self.count()).filter(|t| bits[t / 8] & (1 << (t % 8)) != 0) {
            let (r0, r1, c0, c1) = self.span(t);
            for r in r0..r1 {
                out.extend_from_slice(&cur[r * self.row_bytes + c0..r * self.row_bytes + c1]);
            }
        }
    }

    fn scatter(&self, rows: &[u8], bits: &[u8], cur: &mut [u8]) {
        let mut at = 0;
        for t in (0..self.count()).filter(|t| bits[t / 8] & (1 << (t % 8)) != 0) {
            let (r0, r1, c0, c1) = self.span(t);
            for r in r0..r1 {
                let n = c1 - c0;
                cur[r * self.row_bytes + c0..r * self.row_bytes + c1].copy_from_slice(&rows[at..at + n]);
                at += n;
            }
        }
    }

    /// The bounding rectangle of the change, in rows and whole pixels: (row, byte, rows, bytes).
    fn bounds(&self, cur: &[u8], prev: &[u8], bpp: usize) -> (usize, usize, usize, usize) {
        let (mut top, mut bottom, mut left, mut right) = (usize::MAX, 0, usize::MAX, 0);
        for r in 0..self.rows {
            let (a, b) = (&cur[r * self.row_bytes..(r + 1) * self.row_bytes], &prev[r * self.row_bytes..(r + 1) * self.row_bytes]);
            if a == b {
                continue;
            }
            let first = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(0);
            let last = a.iter().zip(b).rposition(|(x, y)| x != y).unwrap_or(0);
            (top, bottom) = (top.min(r), r + 1);
            left = left.min(first / bpp * bpp);
            right = right.max((last / bpp + 1) * bpp);
        }
        if top == usize::MAX { (0, 0, 0, 0) } else { (top, left, bottom - top, right - left) }
    }

    /// ZW9 (ways register): the changed tiles merged into rectangles, each as (row, byte, rows, bytes).
    /// Changed tiles of one tile row at most `gap` unchanged tiles apart form a run; then any two
    /// rectangles at most `gap` tiles apart in both directions merge into their bounding rectangle,
    /// until no two are that close. The rectangles therefore never overlap.
    fn merged(&self, bits: &[u8], gap: usize) -> Vec<(usize, usize, usize, usize)> {
        let changed = |t: usize| bits[t / 8] & (1 << (t % 8)) != 0;
        // [first tile column, first tile row, last tile column, last tile row]
        let mut rects: Vec<[usize; 4]> = Vec::new();
        for ty in 0..self.down {
            let mut run: Option<[usize; 4]> = None;
            for tx in (0..self.across).filter(|tx| changed(ty * self.across + tx)) {
                match run.as_mut() {
                    Some(r) if tx <= r[2] + gap + 1 => r[2] = tx,
                    _ => {
                        rects.extend(run.take());
                        run = Some([tx, ty, tx, ty]);
                    }
                }
            }
            rects.extend(run);
        }
        let near = |a: &[usize; 4], b: &[usize; 4]| {
            a[0] <= b[2] + gap + 1 && b[0] <= a[2] + gap + 1 && a[1] <= b[3] + gap + 1 && b[1] <= a[3] + gap + 1
        };
        let mut i = 0;
        while i < rects.len() {
            let found = (i + 1..rects.len()).find(|&j| near(&rects[i], &rects[j]));
            if let Some(j) = found {
                let b = rects.swap_remove(j);
                let a = &mut rects[i];
                *a = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                // The grown rectangle may now be near one already passed: start again.
                i = 0;
            } else {
                i += 1;
            }
        }
        rects
            .into_iter()
            .map(|[x0, y0, x1, y1]| {
                let (r0, c0) = (y0 * self.tile_rows, x0 * self.tile_bytes);
                let (r1, c1) = (((y1 + 1) * self.tile_rows).min(self.rows), ((x1 + 1) * self.tile_bytes).min(self.row_bytes));
                (r0, c0, r1 - r0, c1 - c0)
            })
            .collect()
    }
}

/// The way's live part: frames in, clip payloads out.
struct WayEncoder {
    way: Way,
    w: usize,
    packer: Option<Packer>,
    grid: TileGrid,
    view: Vec<u8>,
    prev: Vec<u8>,
    gather: Vec<u8>,
    bits: Vec<u8>,
    out: Vec<u8>,
    /// ZW9, ZW9b: Σ bytes given to zstd, and the most rectangles in one frame.
    rect_src: u64,
    rects_max: usize,
}

impl WayEncoder {
    fn new(way: &Way, w: usize, h: usize, dict: &[u8]) -> Result<Self> {
        let packer = if way.mode == Mode::Raw { None } else { Some(Packer::new(way, dict)?) };
        let size = w * h * way.layout.bytes_per_pixel();
        let gathers = matches!(way.mode, Mode::Tiles | Mode::Rect | Mode::Box | Mode::Switch);
        Ok(Self {
            grid: TileGrid::new(way.layout, w, h, way.tile),
            way: way.clone(),
            w,
            packer,
            view: Vec::with_capacity(if way.layout == Layout::Bgra { 0 } else { size }),
            prev: Vec::new(),
            gather: Vec::with_capacity(if gathers { size } else { 0 }),
            bits: Vec::new(),
            out: Vec::new(),
            rect_src: 0,
            rects_max: 0,
        })
    }

    /// M1: the capture's B,G,R,A frame to the way's layout; the B,G,R,A layout needs none.
    fn convert(&mut self, bgra: &[u8]) {
        if self.way.layout != Layout::Bgra {
            self.way.layout.forward(bgra, self.w, &mut self.view);
        }
    }

    /// Frame `k` (the B,G,R,A capture frame, after `convert`) to its clip payload, left in `self.out`.
    fn encode(&mut self, k: usize, bgra: &[u8]) -> Result<()> {
        let src: &[u8] = if self.way.layout == Layout::Bgra { bgra } else { &self.view };
        let key = self.way.is_key(k);
        let Some(packer) = self.packer.as_mut() else {
            self.out.clear();
            self.out.extend_from_slice(src);
            return Ok(());
        };
        match self.way.mode {
            Mode::Raw => unreachable!("raw has no packer"),
            Mode::Whole => packer.frame(src, None, &mut self.out)?,
            Mode::Stream => packer.chunk(src, self.way.ends_group(k), &mut self.out)?,
            _ if key => packer.frame(src, None, &mut self.out)?,
            Mode::Xor | Mode::Sub => {
                let xor = self.way.mode == Mode::Xor;
                for (p, c) in self.prev.iter_mut().zip(src) {
                    *p = if xor { *p ^ *c } else { c.wrapping_sub(*p) };
                }
                packer.frame(&self.prev, None, &mut self.out)?;
            }
            Mode::Prefix => packer.frame(src, Some(&self.prev), &mut self.out)?,
            Mode::Tiles | Mode::Switch => {
                let changed = self.grid.compare(src, &self.prev, &mut self.bits);
                let share = 100.0 * changed as f64 / self.grid.count() as f64;
                if self.way.mode == Mode::Switch && share >= self.way.theta {
                    packer.frame(src, None, &mut self.out)?;
                    self.out.insert(0, 0);
                } else {
                    self.grid.gather(src, &self.bits, &mut self.gather);
                    packer.frame(&self.gather, None, &mut self.out)?;
                    let head: Vec<u8> = (self.way.mode == Mode::Switch).then_some(1).into_iter().chain(self.bits.iter().copied()).collect();
                    self.out.splice(0..0, head);
                }
            }
            Mode::Rect | Mode::Box => {
                // ZW9: the merged changed-tile rectangles; ZW9b: one global bounding box (none when nothing changed).
                let rects = if self.way.mode == Mode::Rect {
                    self.grid.compare(src, &self.prev, &mut self.bits);
                    self.grid.merged(&self.bits, self.way.gap)
                } else {
                    let bpp = self.way.layout.grid(self.w, 1).2;
                    let rect = self.grid.bounds(src, &self.prev, bpp);
                    if rect.2 * rect.3 == 0 { Vec::new() } else { vec![rect] }
                };
                self.gather.clear();
                for &(row, byte, rows, bytes) in &rects {
                    for r in row..row + rows {
                        let at = r * self.grid.row_bytes + byte;
                        self.gather.extend_from_slice(&src[at..at + bytes]);
                    }
                }
                packer.frame(&self.gather, None, &mut self.out)?;
                self.rect_src += self.gather.len() as u64;
                self.rects_max = self.rects_max.max(rects.len());
                // Header: rectangle count, then (row, byte, rows, bytes) of each, little-endian u32.
                let head: Vec<u8> = std::iter::once(rects.len())
                    .chain(rects.iter().flat_map(|&(a, b, c, d)| [a, b, c, d]))
                    .flat_map(|v| (v as u32).to_le_bytes())
                    .collect();
                self.out.splice(0..0, head);
            }
        }
        if self.way.mode.keeps_previous() && self.way.mode != Mode::Stream {
            self.prev.clear();
            self.prev.extend_from_slice(src);
        }
        if key && self.way.mode == Mode::Switch {
            self.out.insert(0, 0);
        }
        Ok(())
    }

    /// The stream mode's last bytes when the clip stops inside a keyframe group.
    fn finish(&mut self, frames: usize) -> Result<Option<Vec<u8>>> {
        if self.way.mode != Mode::Stream || frames == 0 || self.way.ends_group(frames - 1) {
            return Ok(None);
        }
        let packer = self.packer.as_mut().ok_or("stream without packer")?;
        packer.chunk(&[], true, &mut self.out)?;
        Ok(Some(self.out.clone()))
    }
}

/// The viewer's part: clip payloads back to B,G,R frames.
struct WayDecoder {
    way: Way,
    w: usize,
    unpacker: Unpacker,
    grid: TileGrid,
    cur: Vec<u8>,
    tmp: Vec<u8>,
    bgr: Vec<u8>,
    /// Frames decoded to show the current one alone (seek cost, N_dec).
    chain: usize,
    /// ZW9, ZW9b: Σ rows · bytes of the rectangles listed in the clip's headers.
    listed: u64,
}

impl WayDecoder {
    fn new(way: &Way, w: usize, h: usize, dict: &[u8]) -> Result<Self> {
        let size = w * h * way.layout.bytes_per_pixel();
        Ok(Self {
            unpacker: Unpacker::new(dict)?,
            grid: TileGrid::new(way.layout, w, h, way.tile),
            way: way.clone(),
            w,
            cur: vec![0; size],
            tmp: if way.mode == Mode::Switch || (way.mode.keeps_previous() && way.mode != Mode::Stream && way.kz > 1) {
                vec![0; size]
            } else {
                Vec::new()
            },
            bgr: if way.layout == Layout::Bgr { Vec::new() } else { vec![0; w * h * 3] },
            chain: 0,
            listed: 0,
        })
    }

    /// Clip payload `k` to the next frame; `frame()` then holds it as B,G,R.
    fn decode(&mut self, k: usize, payload: &[u8]) -> Result<()> {
        let size = self.cur.len();
        let key = self.way.is_key(k);
        let mut standalone = key;
        match self.way.mode {
            Mode::Raw => self.cur.copy_from_slice(payload),
            Mode::Whole => {
                self.unpacker.frame(payload, None, &mut self.cur)?;
            }
            Mode::Stream => {
                let n = self.unpacker.chunk(payload, &mut self.cur)?;
                if n != size {
                    return Err(format!("stream chunk {k} gave {n} of {size} bytes"));
                }
            }
            Mode::Switch if payload.first() == Some(&0) => {
                self.unpacker.frame(&payload[1..], None, &mut self.cur)?;
                standalone = true;
            }
            _ if key => {
                self.unpacker.frame(payload, None, &mut self.cur)?;
            }
            Mode::Xor | Mode::Sub => {
                self.tmp.resize(size, 0);
                self.unpacker.frame(payload, None, &mut self.tmp)?;
                let xor = self.way.mode == Mode::Xor;
                for (c, d) in self.cur.iter_mut().zip(&self.tmp) {
                    *c = if xor { *c ^ *d } else { c.wrapping_add(*d) };
                }
            }
            Mode::Prefix => {
                self.tmp.resize(size, 0);
                self.unpacker.frame(payload, Some(&self.cur), &mut self.tmp)?;
                std::mem::swap(&mut self.cur, &mut self.tmp);
            }
            Mode::Tiles | Mode::Switch => {
                let body = if self.way.mode == Mode::Switch { &payload[1..] } else { payload };
                let nbits = self.grid.count().div_ceil(8);
                let (bits, packed) = body.split_at(nbits);
                self.tmp.resize(size, 0);
                let n = self.unpacker.frame(packed, None, &mut self.tmp)?;
                self.grid.scatter(&self.tmp[..n], bits, &mut self.cur);
            }
            Mode::Rect | Mode::Box => {
                let field = |i: usize| u32::from_le_bytes([payload[4 * i], payload[4 * i + 1], payload[4 * i + 2], payload[4 * i + 3]]) as usize;
                let count = field(0);
                let rects: Vec<(usize, usize, usize, usize)> =
                    (0..count).map(|i| (field(1 + 4 * i), field(2 + 4 * i), field(3 + 4 * i), field(4 + 4 * i))).collect();
                let listed: usize = rects.iter().map(|r| r.2 * r.3).sum();
                self.listed += listed as u64;
                self.tmp.resize(size, 0);
                let n = self.unpacker.frame(&payload[4 + 16 * count..], None, &mut self.tmp)?;
                if n != listed {
                    return Err(format!("rectangles of frame {k}: {n} bytes, listed {listed}"));
                }
                let mut at_tmp = 0;
                for (row, byte, rows, bytes) in rects {
                    for r in 0..rows {
                        let at = (row + r) * self.grid.row_bytes + byte;
                        self.cur[at..at + bytes].copy_from_slice(&self.tmp[at_tmp..at_tmp + bytes]);
                        at_tmp += bytes;
                    }
                }
            }
        }
        self.chain = if standalone || !self.way.mode.keeps_previous() { 1 } else { self.chain + 1 };
        if self.way.layout != Layout::Bgr {
            self.way.layout.inverse(&self.cur, self.w, &mut self.bgr);
        }
        Ok(())
    }

    /// The last decoded frame as B,G,R.
    fn frame(&self) -> &[u8] {
        if self.way.layout == Layout::Bgr { &self.cur } else { &self.bgr }
    }
}

/// One corpus folder (`exp_corpus` / `corpus/render.mjs` format): `frame-NNNN.zst`
/// per new frame, zstd of B,G,R rows top to bottom; `frames.jsonl` per instant.
struct Corpus {
    folder: PathBuf,
    w: usize,
    h: usize,
    new_frames: usize,
    /// Every instant of frames.jsonl: (instant, stored frame shown, new).
    instants: Vec<(u64, usize, bool)>,
    d_wait: u64,
    dctx: zstd::bulk::Decompressor<'static>,
    packed: Vec<u8>,
}

impl Corpus {
    fn open(folder: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(folder.join("frames.jsonl")).map_err(|e| format!("frames.jsonl: {e}"))?;
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let first = lines.first().ok_or("frames.jsonl is empty")?;
        let number = |line: &str, key: &str| -> Result<usize> {
            let at = line.find(&format!("\"{key}\":")).ok_or(format!("no {key} in frames.jsonl"))? + key.len() + 3;
            line[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().map_err(|_| format!("bad {key}"))
        };
        let (w, h) = (number(first, "width")?, number(first, "height")?);
        let new_frames = lines.iter().filter(|l| l.contains("\"new\":true")).count();
        // A lost instant names no frame: it has no line in the clip either.
        let instants = lines
            .iter()
            .filter_map(|l| Some((number(l, "instant").ok()? as u64, number(l, "frame").ok()?, l.contains("\"new\":true"))))
            .collect();
        let mut d_wait = 0;
        let mut largest = 0;
        for k in 0..new_frames {
            let size = std::fs::metadata(Self::path_in(folder, k)).map_err(|e| format!("frame {k}: {e}"))?.len();
            d_wait += size;
            largest = largest.max(size);
        }
        Ok(Self {
            folder: folder.to_owned(),
            w,
            h,
            new_frames,
            instants,
            d_wait,
            dctx: zstd::bulk::Decompressor::new().map_err(|e| format!("corpus decoder: {e}"))?,
            packed: Vec::with_capacity(usize::try_from(largest).unwrap_or(0)),
        })
    }

    fn path_in(folder: &Path, k: usize) -> PathBuf {
        folder.join(format!("frame-{k:04}.zst"))
    }

    /// New frame `k` as B,G,R into `out` (w · h · 3 bytes).
    fn frame(&mut self, k: usize, out: &mut [u8]) -> Result<()> {
        self.packed.clear();
        std::fs::File::open(Self::path_in(&self.folder, k))
            .and_then(|mut f| f.read_to_end(&mut self.packed))
            .map_err(|e| format!("corpus frame {k}: {e}"))?;
        let n = self.dctx.decompress_to_buffer(&self.packed, out).map_err(|e| format!("corpus frame {k}: {e}"))?;
        if n != out.len() {
            return Err(format!("corpus frame {k}: {n} bytes, expected {}", out.len()));
        }
        Ok(())
    }
}

/// The trained dictionary of ZW16: the first 20 new frames in the way's layout. The corpus
/// decoding is timed apart (`corpus`); the conversion and the training are the way's work (`train`).
fn train_dictionary(
    way: &Way,
    corpus: &mut Corpus,
    bgr: &mut [u8],
    capture: &mut CaptureFrame,
    corpus_spent: &mut Spent,
    train: &mut Spent,
) -> Result<Vec<u8>> {
    if way.dict_kb == 0 {
        return Ok(Vec::new());
    }
    let w = corpus.w;
    let count = corpus.new_frames.min(DICT_TRAIN_FRAMES);
    // Allocated once at its final size, so no reallocation doubles it for a moment.
    let mut samples = train.time(|| Vec::with_capacity(count * w * corpus.h * way.layout.bytes_per_pixel()));
    let mut sizes = Vec::new();
    let mut view = Vec::new();
    for k in 0..corpus.new_frames.min(DICT_TRAIN_FRAMES) {
        corpus_spent.time(|| corpus.frame(k, bgr))?;
        capture.fill(bgr);
        train.time(|| {
            way.layout.forward(capture.bgra(), w, &mut view);
            samples.extend_from_slice(&view);
            sizes.push(view.len());
        });
    }
    train
        .time(|| zstd::dict::from_continuous(&samples, &sizes, way.dict_kb * 1024))
        .map_err(|e| format!("dictionary training: {e}"))
}

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let folder = PathBuf::from(args.text("folder")?);
    let scratch = PathBuf::from(args.text("out")?);
    let rep: u32 = args.number("rep", 1)?;
    let rate = args.rate()?;
    let mut corpus = Corpus::open(&folder)?;
    // `frames=N`: only the first N new frames (pilot of slow levels; v is per byte). D_wait stays the folder's.
    corpus.new_frames = corpus.new_frames.min(args.number("frames", usize::MAX)?);
    let (w, h) = (corpus.w, corpus.h);
    let px = w * h;
    let way = Way::from_args(&args, px * 3)?;
    let clip = scratch.join(format!("{}-{}-{}-{rep}", folder.file_name().and_then(|n| n.to_str()).unwrap_or("folder"), way.id, std::process::id()));

    // Input preparation (M1, M3): the corpus reader, its B,G,R frame and the capture's B,G,R,A
    // frame exist and have read one frame before M_0, like the product's slot.
    let mut bgr = vec![0u8; px * 3];
    let mut capture = CaptureFrame::new(px);
    let mut corpus_spent = Spent::default();
    if corpus.new_frames > 0 {
        corpus_spent.time(|| corpus.frame(0, &mut bgr))?;
        capture.fill(&bgr);
    }
    let sampler = MemorySampler::start();
    let (m0_ws, _) = memory_now();
    let ram = RamMeter::start();

    // The way's work (M2): conversion, context and compression, writing, dictionary training.
    let (mut convert, mut comp, mut write, mut train) = (Spent::default(), Spent::default(), Spent::default(), Spent::default());
    let wall_start = wall_ns();
    let dict = train_dictionary(&way, &mut corpus, &mut bgr, &mut capture, &mut corpus_spent, &mut train)?;
    let mut files = write.time(|| ClipFiles::create(&clip, w, h))?;
    let mut encoder = comp.time(|| WayEncoder::new(&way, w, h, &dict))?;
    let ldm_read = encoder.packer.as_ref().map_or(0, |p| p.get(sys::ZSTD_cParameter::ZSTD_c_enableLongDistanceMatching));
    if !dict.is_empty() {
        write.time(|| files.frame("dictionary.bin", &dict))?;
    }
    let mut b_max = 0usize;
    let mut last_comp_end = wall_start;
    let mut compressed = 0usize;
    for i in 0..corpus.instants.len() {
        let (instant, frame, new) = corpus.instants[i];
        if frame >= corpus.new_frames {
            continue;
        }
        if new {
            corpus_spent.time(|| corpus.frame(frame, &mut bgr))?;
            capture.fill(&bgr);
            if way.layout != Layout::Bgra {
                convert.time(|| encoder.convert(capture.bgra()));
            }
            comp.time(|| encoder.encode(frame, capture.bgra()))?;
            compressed += 1;
            last_comp_end = wall_ns();
            let name = format!("f-{frame:04}.bin");
            write.time(|| files.frame(&name, &encoder.out))?;
            b_max = b_max.max(encoder.out.len());
        }
        write.time(|| files.line(instant, frame, new))?;
    }
    let cctx_sizeof = encoder.packer.as_ref().map_or(0, Packer::size);
    if let Some(tail) = comp.time(|| encoder.finish(corpus.new_frames))? {
        write.time(|| files.frame("f-end.bin", &tail))?;
    }
    let clip_json = format!("{{{},\"w\":{w},\"h\":{h},\"frames\":{}}}", way.json(), corpus.new_frames);
    let clip_bytes = write.time(|| files.finish(&clip_json))?;
    let t_ready_ns = wall_ns().saturating_sub(last_comp_end);
    let wall_comp_total = wall_ns().saturating_sub(wall_start);
    let ram_read = ram.read();
    let (peak_ws, peak_sampled) = sampler.take_peaks();
    let os_ws = os_peak_working_set();
    let way_spent = convert.plus(comp).plus(write).plus(train);
    let (rect_src, rects_max) = (encoder.rect_src, encoder.rects_max);
    drop(encoder);
    let bytes_out = clip_bytes.total();
    let bytes_disk = ClipFiles::bytes_on_disk(&clip)?;
    // M3: the compress peak is the OS peak of private bytes (the sampler's figure is kept beside it).
    let peak_private = ram_read.peak;

    // Decode and compare every pixel with the corpus.
    let mut dec = Spent::default();
    let mut corpus_dec = Spent::default();
    let mut read = Spent::default();
    sampler.take_peaks();
    let mut decoder = dec.time(|| WayDecoder::new(&way, w, h, &dict))?;
    let mut payload = Vec::with_capacity(b_max);
    let tail = clip.join("f-end.bin");
    let (mut diff_sum, mut diff_frames, mut n_dec_worst, mut decoded) = (0u64, 0u64, 0usize, 0usize);
    for k in 0..corpus.new_frames {
        payload.clear();
        read.time(|| std::fs::File::open(clip.join(format!("f-{k:04}.bin"))).and_then(|mut f| f.read_to_end(&mut payload)))
            .map_err(|e| format!("read clip frame {k}: {e}"))?;
        dec.time(|| decoder.decode(k, &payload))?;
        n_dec_worst = n_dec_worst.max(decoder.chain);
        corpus_dec.time(|| corpus.frame(k, &mut bgr))?;
        let diff: u64 = decoder.frame().iter().zip(&bgr).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
        diff_sum += diff;
        diff_frames += u64::from(diff > 0);
        decoded += 1;
    }
    if tail.exists() {
        payload = std::fs::read(&tail).map_err(|e| format!("read tail: {e}"))?;
        let mut sink = [0u8; 0];
        dec.time(|| decoder.unpacker.chunk(&payload, &mut sink))?;
    }
    let (dec_peak_ws, dec_peak_private) = sampler.take_peaks();
    let rect_listed = decoder.listed;
    drop(decoder);
    if !args.flag("keep", false) {
        let _ = std::fs::remove_dir_all(&clip);
    }

    let (bc, bd) = way.buffers();
    let n = corpus.new_frames.max(1) as u64;
    let folder_name = folder.file_name().and_then(|n| n.to_str()).unwrap_or("");
    println!(
        "{{\"folder\":\"{folder_name}\",{},\"rep\":{rep},\"w\":{w},\"h\":{h},\"n_new\":{},\"d_wait\":{},\"frames\":{},\"bytes_out\":{bytes_out},\"bytes_disk\":{bytes_disk},\"bytes_per_frame\":{},{},\
\"input\":\"bgra\",{},{},{},{},{},{},\"comp_cpu_per_frame_ns\":{},\"way_cpu_per_frame_ns\":{},\"comp_wall_total_ns\":{wall_comp_total},\"dict_bytes\":{},{},\
{},\"m0_ws\":{m0_ws},\"peak_private\":{peak_private},\"peak_private_sampled\":{peak_sampled},\"b_max\":{b_max},\"peak_ws\":{peak_ws},\"os_peak_ws\":{os_ws},\
\"m_cctx_est\":{},\"cctx_sizeof\":{cctx_sizeof},\"bc\":{bc},\"m_dctx_est\":{},\"bd\":{bd},\"ldm_read\":{ldm_read},\"rect_src_bytes\":{rect_src},\"rect_listed_bytes\":{rect_listed},\"rects_max\":{rects_max},\
{},{},{},\"dec_cpu_per_frame_ns\":{},\"dec_peak_private\":{dec_peak_private},\"dec_peak_ws\":{dec_peak_ws},\
\"frames_decoded\":{decoded},\"diff_sum\":{diff_sum},\"diff_frames\":{diff_frames},\"n_dec_worst\":{n_dec_worst},\"t_ready_ns\":{t_ready_ns}}}",
        way.json(),
        corpus.new_frames,
        corpus.d_wait,
        compressed,
        bytes_out / n,
        clip_bytes.json(),
        convert.json("convert", rate),
        comp.json("comp", rate),
        write.json("write", rate),
        train.json("train", rate),
        way_spent.json("way", rate),
        corpus_spent.json("corpus", rate),
        comp.cpu_from_cycles(rate) / n,
        way_spent.cpu_from_cycles(rate) / n,
        dict.len(),
        format_args!("\"px\":{px}"),
        ram_read.json(""),
        if way.mode == Mode::Raw { 0 } else { Estimate::cctx(&way, px * way.layout.bytes_per_pixel(), dict.len()) },
        if way.mode == Mode::Raw { 0 } else { Estimate::dctx(&way, dict.len()) },
        dec.json("dec", rate),
        read.json("read", rate),
        corpus_dec.json("corpus_dec", rate),
        dec.cpu_from_cycles(rate) / n,
    );
    Ok(())
}
