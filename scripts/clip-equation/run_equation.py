"""Run the clip recorder equation for the development PC and the weakest machine.

python -I run_equation.py
Weakest machine (owner): Core i3 2.2 GHz, 2 cores / 4 threads, 4 GB, Intel graphics (Core i3-2330M class).
Processor costs are measured on the development PC (Core i7-13620H, 16 threads). The weakest-machine
figures are those costs times the PassMark single-thread ratio (equation section 6); they stay
estimates until a run on a real Core i3. Prints markdown tables.

Sources: 2026-11-21 bench.rs and measure_encoder.ps1 (c_conv, c_enc through ffmpeg, bytes);
2026-11-22 package 0 probe in probe/ (results-package0.md): c_read, c_obs, c_disk, f_o, n_new (M1 to M5),
c_enc through the Media Foundation sink writer (M8, synthetic frames, shown for comparison only).
"""
SPEED_RATIO = 3546 / 964          # PassMark single thread: i7-13620H / i3-2330M
F = 20
CLIP_S = 60
U_MAX = 0.05

# (name, logical processors, cost multiplier, observation rate f_o when the window changes every refresh)
# f_o: development PC with REQUESTED_UPDATE_INTERVAL_MS = 11 on its 144 Hz panel delivered 72 per second (M3);
# the weakest machine runs Windows 10 (interval at Default) on a 60 Hz panel: at most 60 per second.
MACHINES = [("development PC", 16, 1.0, 72.0), ("weakest machine (estimate)", 4, SPEED_RATIO, 60.0)]

# --- package 0, development PC, 2026-11-22 ---
# c_read, ns per pixel: GPU copy into a staging texture + Map + tight row copy; whole-process processor time;
# Intel UHD Graphics (the adapter windows-capture's own device lands on); blocking Map; higher of two runs (M1).
C_READ = {854 * 480: 0.456, 1280 * 720: 0.330, 1920 * 1080: 0.349}
# c_obs, ms per delivered frame: GPU copy into a slot + composition time, no read-back (M2).
C_OBS_MS = 0.373                  # whole process minus painter minus idle baseline (driver threads included)
C_OBS_THREAD_MS = 0.245           # the capture thread alone
# c_disk, ns per pixel: one raw BGRA frame written as one file, whole-process processor time (M5).
C_DISK_BUFFERED = 1.507           # through the file cache at 20 frames per second (the worst live rate)
C_DISK_UNBUFFERED = 0.756         # FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH
# c_enc through the Media Foundation sink writer, ns per pixel, 1280 x 720 synthetic text-like frames (M8).
C_ENC_MF_SW, C_ENC_MF_QSV = 4.87, 1.45

# --- 2026-11-21 (equation section 6) ---
C_CONV, C_DIFF = 1.65, 1.75       # bench.rs
C_ENC_SW, C_ENC_HW = 7.0, 2.0     # Windows software H.264 encoder / Intel Quick Sync through ffmpeg, real screen clips
K_TEXT, P_TEXT = 125_524 / (1252 * 890), 32_500 / (1252 * 890)   # bytes per pixel, software encoder, quality 70
K_STILL = 41_505 / (1502 * 1038)

SIZES = [("854 x 480", 854 * 480), ("1280 x 720", 1280 * 720), ("1920 x 1080", 1920 * 1080)]
# (activity, delivered frames per second or None for the machine's f_o, new frames per second n_new)
ACTIVITY = [
    ("caret blinking (M4)", 1.67, 1.67),
    ("changing text window (equation, 3.8/s)", 3.83, 3.83),
    ("typing 8 chars/s (M4)", 8.0, 8.0),
    ("mouse over window, cursor in frames (M4)", None, 20.0),
    ("changes every refresh (M3 counter window)", None, 20.0),
]
PATHS = [("software encoder", C_CONV + C_ENC_SW), ("Quick Sync, conversion on the processor", C_CONV + C_ENC_HW),
         ("Quick Sync, conversion on the graphics chip", C_ENC_HW)]


def a_rt(px, delivered, n_new, ratio, c_disk=C_DISK_BUFFERED):
    """Live part, thread-seconds per second: f_o * c_obs + n_new * px * (c_read + c_disk)."""
    return ratio * (delivered * C_OBS_MS * 1e-3 + n_new * px * (C_READ[px] + c_disk) * 1e-9)


def delivered_rate(delivered, f_o):
    return f_o if delivered is None else delivered


print(f"speed ratio {SPEED_RATIO:.2f}\n")
print("| Measured cost (development PC) | Value | Weakest machine (estimate) |\n|---|---|---|")
for name, value, unit in [("c_obs, whole process", C_OBS_MS, "ms per delivered frame"),
                          ("c_obs, capture thread only", C_OBS_THREAD_MS, "ms per delivered frame"),
                          ("c_read 854 x 480", C_READ[854 * 480], "ns per pixel"),
                          ("c_read 1280 x 720", C_READ[1280 * 720], "ns per pixel"),
                          ("c_read 1920 x 1080", C_READ[1920 * 1080], "ns per pixel"),
                          ("c_disk, through the file cache", C_DISK_BUFFERED, "ns per pixel"),
                          ("c_disk, no cache, write-through", C_DISK_UNBUFFERED, "ns per pixel"),
                          ("c_enc, Media Foundation software (synthetic)", C_ENC_MF_SW, "ns per pixel"),
                          ("c_enc, Media Foundation Quick Sync (synthetic)", C_ENC_MF_QSV, "ns per pixel")]:
    print(f"| {name} | {value:.3f} {unit} | {value * SPEED_RATIO:.3f} {unit} |")

print("\n**M9: live share and the worker's remainder (5 % minus the live share)**\n")
print("| Machine | Window | Activity | Live share | Worker cap |\n|---|---|---|---|---|")
for machine, threads, ratio, f_o in MACHINES:
    for size, px in SIZES:
        for act, delivered, n_new in ACTIVITY:
            share = a_rt(px, delivered_rate(delivered, f_o), n_new, ratio) / threads
            cap = U_MAX - share
            cap_text = f"{cap * 100:.2f} %" if cap > 0 else "none: live part alone above 5 % (E1 load shedding)"
            print(f"| {machine} | {size} | {act} | {share * 100:.2f} % | {cap_text} |")

print("\n**Live share with unbuffered raw writes (c_disk without the file cache)**\n")
print("| Machine | Window | Activity | Live share | Worker cap |\n|---|---|---|---|---|")
for machine, threads, ratio, f_o in MACHINES:
    for size, px in SIZES[1:]:
        for act, delivered, n_new in ACTIVITY[2:]:
            share = a_rt(px, delivered_rate(delivered, f_o), n_new, ratio, C_DISK_UNBUFFERED) / threads
            cap = U_MAX - share
            cap_text = f"{cap * 100:.2f} %" if cap > 0 else "none (E1)"
            print(f"| {machine} | {size} | {act} | {share * 100:.2f} % | {cap_text} |")

print("\n**Raw write volume (E8): n_new * width * height * 4 bytes**\n")
print("| Window | Activity | MB per second | GB per hour |\n|---|---|---|---|")
for size, px in SIZES:
    for act, _, n_new in ACTIVITY:
        rate = n_new * px * 4
        print(f"| {size} | {act} | {rate / 1e6:.1f} | {rate * 3600 / 1e9:.0f} |")

for path, c in PATHS:
    print(f"\n**Weakest machine (estimate), {path}: time until a 60 s clip is ready**\n")
    print("| Window | Activity | Live share | Deferred work if done live | Ready after |\n|---|---|---|---|---|")
    threads, ratio, f_o = 4, SPEED_RATIO, 60.0
    budget = U_MAX * threads
    for size, px in SIZES:
        for act, delivered, n_new in ACTIVITY:
            live = a_rt(px, delivered_rate(delivered, f_o), n_new, ratio)
            work = n_new * px * c * ratio * 1e-9
            if live >= budget:
                ready = "never within 5 %: live part alone uses the budget"
            else:
                ready = f"{max(CLIP_S, CLIP_S * work / (budget - live)):.0f} s"
            print(f"| {size} | {act} | {live / threads * 100:.2f} % | {work / threads * 100:.1f} % | {ready} |")

print("\n**Bytes, Windows software encoder at quality 70 (2026-11-21)**\n")
print("| Window | Content | Keyframe every 1 s | Keyframe every 5 s |\n|---|---|---|---|")
for size, px in SIZES + [("1252 x 890 (measured)", 1252 * 890)]:
    for content, k, p, n_new in [("small changing text", K_TEXT, P_TEXT, 3.83), ("still window", K_STILL, 0.0, 0.0)]:
        row = [60 * (k * px / tau + n_new * p * px) / 1e6 for tau in (1, 5)]
        print(f"| {size} | {content} | {row[0]:.1f} MB/min | {row[1]:.1f} MB/min |")
