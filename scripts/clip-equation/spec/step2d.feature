# Experiments steps 2 and 3, way D: zstd -3 waiting frames, then a worker encodes H.264.
# Spec: docs/clip-recorder-experiments-plan.md, "Equations" 7, "Factors" D1 to D5,
# "Acceptance (BDD)" rows marked D of steps 2 and 3; docs/clip-recorder-ways.md DW1 to DW4.
# Checked by: node scripts/clip-equation/spec/run-spec.mjs step2d <d-runs.csv>
# Symbols: N_new = new frames of the corpus folder, N_enc = frames in the clip,
# tau_f = GOP in encoded frames, q = quality level, beta = bitrate fallback.

Feature: Way D encodes every new frame of the corpus with every H.264 encoder

  Scenario: D: encoders
    Given the H.264 encoders of results/encoders.json counted unique by name and vendor
    When each one is activated from its enumeration object
    Then the count that activates is 4
    And an encoder that fails is reported with its HRESULT
    # Equation: count(activated) | Known: 4

  Scenario: D: frames encoded
    Given a finished run on one corpus folder
    Then N_enc read back from the clip equals N_new of the folder
    # Equation: N_enc = N_new | Known: exact, every run

  Scenario: D: keyframes
    Given a finished run with GOP tau_f
    Then the keyframes read back from the clip are ceil(N_enc / tau_f)
    # Equation: N_key = ceil(N_enc / tau_f) | Known: exact for tau_f in {20, 40, 100, 200}

  Scenario: D: B-frames
    Given a finished run
    Then no slice of the clip is a B slice
    # Equation: count(B) | Known: 0

  Scenario: D: meter self-check
    Given a corpus frame x
    Then SSIM-Y(x, x) is 1, PSNR-Y(x, x) is infinite and R(x, x) is 1
    # Equation: SSIM-Y(x, x); PSNR-Y(x, x); R(x, x) | Known: 1; inf; 1

  Scenario: D: known distortion
    Given a corpus frame x and x' with Y + 4 on every pixel and no clipping
    Then PSNR-Y(x, x') is 20 * log10(255 / 4) within 0.01 dB
    # Equation: MSE = d^2, PSNR = 20 * log10(255 / d), d = 4 | Known: 36.0896 dB within 0.01 dB

  # Measurement rules M1 to M7 and M9 (plan, "Measurement rules, the same for both ways"; issue #295).

  Scenario: D: live and worker are two processes
    Given a finished run
    Then the live part and the worker ran as two processes with different ids
    And the way's CPU is the sum of both processes' cycle counters
    # Equation: pid_live != pid_worker; way_cpu_ns = a_rt_ns + a_w_ns | Known: every run (M2, M3)

  Scenario: D: RAM base excludes corpus buffers
    Given a finished run whose input was prepared as B, G, R, A before M_0
    Then each phase set its own OS peak above the peak at M_0
    # Equation: input = bgra; os_peak_private > os_peak_at_m0 for live and worker | Known: every run (M1, M3, #5)

  Scenario: D: RAM is the OS peak of both phases
    Given a finished run
    Then RAM_D is the live OS peak above its M_0 plus the worker OS peak above its M_0
    # Equation: ram_os = live_ram_os + worker_ram_os; m_live = live_ram_os; m_worker = worker_ram_os | Known: every run (M3, #11)

  Scenario: D: bytes include frames.jsonl and clip.json
    Given a finished run
    Then the clip's bytes are the MP4, frames.jsonl with one line per instant, and clip.json, equal to the folder on disk
    # Equation: bytes_disk = frame_bytes + index_bytes + clip_json_bytes = on-disk sum; frame_bytes = MP4; index_lines = N_inst | Known: exact, every run (M4, #6)

  Scenario: D: a refused setting is recorded as refused
    Given a run with requested and read-back q, rate mode, GOP and hint
    Then a run whose read-back differs has status setting_refused:<name> and its key columns hold the read-back values
    # Equation: status = setting_refused:<first differing name> iff read-back != requested | Known: every run (M5, #7)

  Scenario: D: beta never 0
    Given every planned and every finished run
    Then beta is above 0, and quality mode keeps the product fallback 0.10
    # Equation: beta > 0; rate = quality implies beta = FALLBACK_BITS_PER_PIXEL_FRAME = 0.10 | Known: every run (M5, #7)

  Scenario: D: every decoded frame mapped by time
    Given a run decoded from its clip
    Then every decoded frame maps to one new frame within half an instant, and a run with a missing, extra or mistimed frame has status frame_map
    # Equation: frames_mapped = N_new; missing = extra = mistimed = 0, else status frame_map | Known: every run (M6, #9)

  Scenario: D: R n/a on empty source text
    Given the fixed text regions of corpus.json for every text scenario
    Then R is read on those regions, and R is n/a when the source text read is empty or the scenario has no text
    # Equation: r = n/a iff r_source_chars = 0; r_regions = regions of the scenario | Known: every run (M7, #14)

  Scenario: D: PSNR inf at zero MSE
    Given a finished run
    Then PSNR-Y is computed from the clip's total MSE of Y, inf when that MSE is 0
    # Equation: psnr_global = 10 * log10(255^2 / mse_y), inf at mse_y = 0 | Known: within 1e-6 dB, every run (M7, #22)

  Scenario: D: run list with unique keys
    Given results/d-runlist.json written by run-d.mjs
    Then it holds grid A-D and the D part of grid B with every key resolved, and no key twice
    # Equation: runs = count(plan A) + count(plan B) = 2808; unique keys = runs | Known: exact (M9)

  Scenario: D: bytes against the prior
    Given a finished run in quality mode
    Then its bytes per minute are printed beside the prior and the error
    # Equation: prior B_min = 10.3 MB * px_out / (1252 * 890) (Quick Sync, q 70, docs/clip-recorder-equation.md section 6) | Known: reported, no pass or fail before the fit

  Scenario: D: CPU against the prior
    Given a finished run
    Then its worker CPU is printed beside the prior and the error
    # Equation: prior a_w = N_new * px_out * (c_conv + c_enc(E)), c_conv 1.65 ns, c_enc 7.0 ns (software) or 2.0 ns (hardware) per pixel | Known: reported, no pass or fail before the fit

  Scenario: D: RAM against the prior
    Given a finished run
    Then its worker peak RAM is printed beside the prior and the error
    # Equation: prior M_worker = px_out * (3 + 1.5), one frame held (d = 1), M0_w and M_enc not yet known | Known: reported, no pass or fail before the fit
