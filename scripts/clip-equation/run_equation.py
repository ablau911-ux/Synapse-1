"""Run the clip recorder equation for the weakest machine (Core i3 2.2 GHz, 2 cores / 4 threads, 4 GB, Intel graphics).

python -I run_equation.py
Constants measured on the development PC (Core i7-13620H) on 2026-11-21, scaled by the PassMark
single-thread ratio. Prints markdown tables.
"""
SPEED_RATIO = 3546 / 964          # PassMark single thread: i7-13620H / i3-2330M
THREADS = 4
BUDGET = 0.05 * THREADS           # thread-seconds per second the recorder may use
CLIP_S = 60
F = 20

# processor time per pixel on the development PC, nanoseconds (bench.rs, measure_encoder.ps1)
C_READ, C_CONV, C_DIFF = 0.13, 1.65, 1.75
C_ENC_SW, C_ENC_HW = 7.0, 2.0     # Windows software H.264 encoder / Intel Quick Sync, per frame handed over

# bytes per pixel, Windows software H.264 encoder at quality 70 (measured clips)
K_TEXT, P_TEXT = 125_524 / (1252 * 890), 32_500 / (1252 * 890)
K_STILL = 41_505 / (1502 * 1038)

SIZES = [("854 x 480", 854 * 480), ("1280 x 720", 1280 * 720), ("1920 x 1080", 1920 * 1080)]
ACTIVITY = [("typical (3.8 new frames/s)", 3.83), ("worst (20 new frames/s)", 20.0)]
PATHS = [("software encoder", C_CONV + C_ENC_SW), ("Quick Sync, conversion on the processor", C_CONV + C_ENC_HW),
         ("Quick Sync, conversion on the graphics chip", C_ENC_HW)]


def ns(c):
    return c * SPEED_RATIO * 1e-9


print(f"speed ratio {SPEED_RATIO:.2f}; budget {BUDGET:.2f} thread-seconds per second\n")
print("| Cost on the weakest machine | ns per pixel | ms per 1920 x 1080 frame |\n|---|---|---|")
for name, c in [("read a frame back", C_READ), ("convert colours (code as built)", C_CONV), ("compare 8x8 blocks", C_DIFF),
                ("encode, Windows software encoder", C_ENC_SW), ("encode, Quick Sync (processor part)", C_ENC_HW)]:
    print(f"| {name} | {c * SPEED_RATIO:.1f} | {c * SPEED_RATIO * 1920 * 1080 / 1e6:.0f} |")

for path, c in PATHS:
    print(f"\n**{path}**\n\n| Window | Activity | Processor share if done live | 60 s clip ready after |\n|---|---|---|---|")
    for size, px in SIZES:
        for act, n_new in ACTIVITY:
            a_rt = n_new * px * ns(C_READ)
            work = n_new * px * ns(c)
            live_share = (a_rt + work) / THREADS
            ready = max(CLIP_S, CLIP_S * work / (BUDGET - a_rt))
            print(f"| {size} | {act} | {live_share * 100:.1f} % | {ready:.0f} s |")

print("\n**Repeated frames handed to the software encoder on every tick (as built)**\n")
for size, px in SIZES:
    print(f"- {size}: {F * px * ns(C_ENC_SW) / THREADS * 100:.0f} % of the processor")

print("\n**Bytes, Windows software encoder at quality 70**\n\n| Window | Content | Keyframe every 1 s | Keyframe every 5 s |\n|---|---|---|---|")
for size, px in SIZES + [("1252 x 890 (measured)", 1252 * 890)]:
    for content, k, p, n_new in [("small changing text", K_TEXT, P_TEXT, 3.83), ("still window", K_STILL, 0.0, 0.0)]:
        row = [60 * (k * px / tau + n_new * p * px) / 1e6 for tau in (1, 5)]
        print(f"| {size} | {content} | {row[0]:.1f} MB/min | {row[1]:.1f} MB/min |")

px = 1920 * 1080
print(f"\nframe buffers at 1920 x 1080: {(4 * px * 4 + px * 1.5) / 1e6:.0f} MB "
      f"({(4 * px * 4 + px * 1.5) / 4e9 * 100:.1f} % of 4 GB)")
waiting = 1200 * (1 - (BUDGET - 20 * px * ns(C_READ)) / (20 * px * ns(C_CONV + C_ENC_SW)))
print(f"worst case spool (1920 x 1080, 20 new frames/s, 60 s, software): {waiting:.0f} frames waiting, "
      f"{waiting * px * 4 / 1e9:.1f} GB raw, written at {20 * px * 4 / 1e6:.0f} MB/s")
