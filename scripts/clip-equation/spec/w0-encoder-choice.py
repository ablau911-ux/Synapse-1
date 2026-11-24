# Plan 5.0, W0 part 1: which AV1 encoder library the probe should link.
# The three candidates are measured through ffmpeg, which carries the same libraries, on the same
# lossless captures, with one key picture every 5 s (KEY_INSTANTS = 100). One run each.
# A candidate passes when its file is within 20 % of libaom's at a closeness that is not lower.
# usage: python -I w0-encoder-choice.py <folder with *.src.mkv> <output json>
import os, re, subprocess, sys, json

SRC_DIR, OUT_JSON = sys.argv[1], sys.argv[2]
FPS = 20
KEY = ["-g", "100"]
LOW = 0x00004000  # below-normal priority
def ff(args, timeout=1800):
    p = subprocess.run(["ffmpeg", "-hide_banner", "-nostdin", "-y", "-benchmark", *args], capture_output=True, text=True, errors="replace",
                       timeout=timeout, creationflags=LOW)
    return p.returncode, p.stderr
def bench(err):
    m = re.search(r"utime=([\d.]+)s stime=([\d.]+)s rtime=([\d.]+)s", err or "")
    r = re.search(r"maxrss=(\d+)KiB", err or "")
    return ((float(m[1]) + float(m[2]), float(m[3])) if m else (None, None)), (int(r[1]) / 1024 if r else None)
def frames_of(path):
    return int(subprocess.run(["ffprobe", "-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets",
                               "-of", "csv=p=0", path], capture_output=True, text=True).stdout.strip())
def ssim(out, src):
    c, e = ff(["-i", out, "-i", src, "-lavfi", "[0:v]format=gbrp[a];[1:v]format=gbrp[b];[a][b]ssim", "-f", "null", "-"])
    m = re.search(r"All:([\d.]+)", e or ""); return float(m[1]) if m else None

CANDIDATES = [
    ("libaom, realtime, screen", lambda q: ["-c:v", "libaom-av1", "-usage", "realtime", "-cpu-used", "8", "-crf", str(q), "-b:v", "0", *KEY,
                                            "-aom-params", "tune-content=screen", "-row-mt", "1", "-pix_fmt", "yuv420p"], [35, 25]),
    ("rav1e, speed 10", lambda q: ["-c:v", "librav1e", "-speed", "10", "-qp", str(q), *KEY, "-tiles", "4", "-pix_fmt", "yuv420p"], [140, 110, 80]),
    ("rav1e, speed 10, low latency", lambda q: ["-c:v", "librav1e", "-speed", "10", "-qp", str(q), *KEY, "-tiles", "4", "-rav1e-params", "low_latency=true",
                                                "-pix_fmt", "yuv420p"], [110]),
    ("SVT-AV1, preset 10, screen", lambda q: ["-c:v", "libsvtav1", "-preset", "10", "-crf", str(q), *KEY, "-pix_fmt", "yuv420p", "-svtav1-params", "scm=1"], [35, 25]),
    ("SVT-AV1, preset 12, screen", lambda q: ["-c:v", "libsvtav1", "-preset", "12", "-crf", str(q), *KEY, "-pix_fmt", "yuv420p", "-svtav1-params", "scm=1"], [35]),
]
rows = []
for scene in ["scroll-1920x1080", "mix-1920x1080", "real-youtube-even"]:
    src = os.path.join(SRC_DIR, scene + ".src.mkv"); secs = frames_of(src) / FPS
    c, e = ff(["-i", src, "-f", "null", "-"]); (base, _), _ = bench(e)
    for name, mk, qs in CANDIDATES:
        for q in qs:
            out = os.path.join(SRC_DIR, "w0.tmp.mkv")
            c, e = ff(["-i", src, *mk(q), out])
            if c != 0 or not os.path.exists(out) or os.path.getsize(out) < 1000:
                why = [l for l in (e or "").splitlines() if re.search(r"error|fail|unsupported|not support|invalid", l, re.I)]
                row = {"scene": scene, "candidate": name, "q": q, "failed": (why[0][-150:] if why else "failed")}
            else:
                (cpu, rtime), rss = bench(e)
                row = {"scene": scene, "candidate": name, "q": q, "mb_per_min": os.path.getsize(out) / 1048576 * 60 / secs, "ssim": ssim(out, src),
                       "cpu_per_sec": ((cpu - (base or 0)) / secs) if cpu is not None else None, "ram_mb": rss,
                       "speed_x": (secs / rtime) if rtime else None}
            rows.append(row); print(json.dumps(row), flush=True)
json.dump(rows, open(OUT_JSON, "w"), indent=1)
print("done")
