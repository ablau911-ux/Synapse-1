# Plan 5.0: do tile columns make a picture late in a part faster to fetch, and what do they cost in size?
# libaom AV1, realtime, screen content, quality 35, key picture every 200 pictures; decoded with libaom's
# own decoder (the one the product uses). One run each.
# usage: python -I w0-tiles.py <folder with *.src.mkv> <output json>
import os, re, subprocess, sys, json, time

SRC_DIR, OUT_JSON = sys.argv[1], sys.argv[2]
FPS, KEY = 20, 200
def run(args):
    t = time.perf_counter()
    p = subprocess.run(["ffmpeg", "-hide_banner", "-nostdin", "-y", *args], capture_output=True, text=True, errors="replace", timeout=1800)
    return p.returncode, p.stderr, (time.perf_counter() - t) * 1000
def ssim(out, src):
    c, e, _ = run(["-i", out, "-i", src, "-lavfi", "[0:v]format=gbrp[a];[1:v]format=gbrp[b];[a][b]ssim", "-f", "null", "-"])
    m = re.search(r"All:([\d.]+)", e or ""); return float(m[1]) if m else None
rows = []
for scene in ["real-youtube-even", "scroll-1920x1080"]:
    src = os.path.join(SRC_DIR, scene + ".src.mkv")
    for cols in [0, 1, 2]:  # log2 of the number of tile columns: 1, 2, 4 columns
        out = os.path.join(SRC_DIR, f"w0.tiles{cols}.ivf")
        c, e, enc_ms = run(["-i", src, "-c:v", "libaom-av1", "-usage", "realtime", "-cpu-used", "8", "-crf", "35", "-b:v", "0", "-g", str(KEY),
                            "-aom-params", "tune-content=screen", "-row-mt", "1", "-tile-columns", str(cols), "-pix_fmt", "yuv420p", "-frames:v", str(KEY), out])
        if c != 0: rows.append({"scene": scene, "tile_columns": 1 << cols, "failed": e[-200:]}); print(rows[-1]); continue
        row = {"scene": scene, "tile_columns": 1 << cols, "mb_per_min": os.path.getsize(out) / 1048576 * 60 / (KEY / FPS), "ssim": ssim(out, src), "encode_ms": round(enc_ms)}
        for threads in [1, 16]:
            # decode the whole part (200 pictures) with libaom's decoder: the worst fetch
            c, e, ms = run(["-threads", str(threads), "-c:v", "libaom-av1", "-i", out, "-f", "null", "-"])
            row[f"decode_all_ms_threads{threads}"] = round(ms) if c == 0 else "failed: " + e[-120:]
        rows.append(row); print(json.dumps(row), flush=True)
        os.remove(out)
json.dump(rows, open(OUT_JSON, "w"), indent=1)
