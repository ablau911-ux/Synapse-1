# Plan 5.0, W0: what does the distance between key pictures cost and buy, for libaom AV1 in realtime screen mode?
# For each distance: file size, closeness, processor time, and the time to fetch one picture at the worst place
# (the last picture before the next key). One run each.
# usage: python -I w0-key-distance.py <folder with *.src.mkv> <output json>
import os, re, subprocess, sys, json, time

SRC_DIR, OUT_JSON = sys.argv[1], sys.argv[2]
FPS = 20
LOW = 0x00004000
def ff(args, low=True, timeout=1800):
    t = time.perf_counter()
    p = subprocess.run(["ffmpeg", "-hide_banner", "-nostdin", "-y", "-benchmark", *args], capture_output=True, text=True, errors="replace",
                       timeout=timeout, creationflags=LOW if low else 0)
    return p.returncode, p.stderr, (time.perf_counter() - t) * 1000
def cpu_of(err):
    m = re.search(r"utime=([\d.]+)s stime=([\d.]+)s", err or ""); return (float(m[1]) + float(m[2])) if m else None
def frames_of(path):
    return int(subprocess.run(["ffprobe", "-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets",
                               "-of", "csv=p=0", path], capture_output=True, text=True).stdout.strip())
def ssim(out, src):
    c, e, _ = ff(["-i", out, "-i", src, "-lavfi", "[0:v]format=gbrp[a];[1:v]format=gbrp[b];[a][b]ssim", "-f", "null", "-"])
    m = re.search(r"All:([\d.]+)", e or ""); return float(m[1]) if m else None
rows = []
for scene in ["real-youtube-even", "scroll-1920x1080", "mix-1920x1080"]:
    src = os.path.join(SRC_DIR, scene + ".src.mkv"); n = frames_of(src); secs = n / FPS
    c, e, _ = ff(["-i", src, "-f", "null", "-"]); base = cpu_of(e) or 0.0
    for key in [100, 200, 300, 600]:
        out = os.path.join(SRC_DIR, f"w0.key{key}.mkv")
        c, e, _ = ff(["-i", src, "-c:v", "libaom-av1", "-usage", "realtime", "-cpu-used", "8", "-crf", "35", "-b:v", "0", "-g", str(key),
                      "-aom-params", "tune-content=screen", "-row-mt", "1", "-pix_fmt", "yuv420p", out])
        cpu = cpu_of(e)
        # worst place to fetch: the last picture before the second key picture (or the last picture of the clip)
        worst = min(key - 1, n - 1) / FPS
        times = []
        for t in [worst, min(worst, secs - 0.05)]:
            c2, e2, ms = ff(["-ss", f"{t:.3f}", "-i", out, "-frames:v", "1", "-f", "null", "-"], low=False); times.append(round(ms))
        row = {"scene": scene, "key_pictures_every_s": key / FPS, "mb_per_min": os.path.getsize(out) / 1048576 * 60 / secs, "ssim": ssim(out, src),
               "cpu_per_sec": ((cpu - base) / secs) if cpu is not None else None, "worst_fetch_ms": max(times)}
        rows.append(row); print(json.dumps(row), flush=True)
        os.remove(out)
json.dump(rows, open(OUT_JSON, "w"), indent=1); print("done")
