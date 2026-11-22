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
