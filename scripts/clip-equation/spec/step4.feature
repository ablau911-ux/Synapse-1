# Experiments step 4: fit and validate the equations.
# Spec: docs/clip-recorder-experiments-plan.md, "Equations" 1 to 7, "How absolute and predictable is reached" 3,
# "Run design" (run list v2, hold-out), "Procedure" 4, "Acceptance (BDD)" step 4.
# Checked by: node scripts/clip-equation/spec/run-spec.mjs step4 [results/fit.json]
# Built by:   node scripts/clip-equation/fit/fit.mjs --z <z CSVs> --d <d CSVs>
# A row that needs data the pilot does not have prints PENDING ("pending: full run"): neither a pass nor a miss.

Feature: Every equation of the plan is fitted, and the held-out runs are predicted

  # Rows of issue #295 (plan "Measurement rules" M5, M7, M10), on synthetic fixtures in spec/step4.fixtures.mjs.

  Scenario: #8 failed repeats, missing R and refused settings block a configuration
    Given D configurations with one failed repeat, one repeat without R and one refused setting (status setting_refused:q)
    Then each is dropped and out of the fit, and the failure counts are kept
    # Equation: eligible = every repeat ok and R read on every repeat of a text scenario | Known: 3 dropped, 4 failed runs, 3 blocked configurations

  Scenario: #10 M1 FULL only when the resolved keys match both run plans, without duplicates
    Given a Z run list and a D run plan with fixture rows: complete; duplicates; a row with its cell id but other options; rows from two run lists; a D run missing; no D plan
    Then only the complete case is labeled FULL
    # Equation: FULL = (resolved Z keys = planned Z keys) and (resolved D keys = planned D keys) and duplicates = 0 and one run list hash | Known: FULL, then not FULL in all 7 other cases

  Scenario: H1 every option the runner keys on is in the configuration key (ZW9 gap)
    Given ZW9 rows with merge gap 0, 1 and 2, three repeats each
    Then they are three configurations of three repeats
    # Equation: key columns = the CSV's option columns (before rep) and the configured list | Known: 3 x 3

  Scenario: H2 bytes are every retained file for both ways; payload fits add the metadata
    Given D rows with an MP4 payload and bytes_disk, and Z rows with frame_bytes and bytes_disk
    Then bytes per minute come from bytes_disk for both ways, zeta from the Z payload, and D held-out bytes are judged on bytes_disk
    # Equation: B_min = (payload * N_new / frames + metadata) * 60 / t | Known: within 1e-9

  Scenario: H5 eligibility by requested configuration: every repeat present and ok
    Given a requested configuration with one repeat refused (recorded under its actual q), one with a repeat missing, and a complete one
    Then the first two are blocked, the refused repeat is listed under its actual settings, the complete one is eligible
    # Equation: eligible(requested) = every repeat 1..R present with status ok | Known: blocked, listed, blocked, eligible

  Scenario: H5 a refusal into a valid group leaves its O, J and text verdicts unchanged
    Given typing, scroll, ui and flash valid at q 40, and a typing repeat requested at q 50 that was refused and recorded under q 40
    Then the refused run is listed apart with a cfg of its own, and every valid configuration has the same O, clause and J, and the best way is the same, with and without it
    # Equation: O, J and best = f(valid configurations only) | Known: flash q40 kept; nothing changed

  Scenario: H6 validity checks block, known-result misses are reported
    Given D rows with pass_keyframes = 0 (#297), D rows with status ocr_error, and Z rows with pass_ramOs = 0
    Then the keyframe miss is reported and the configuration stays eligible; the OCR error and the RAM-base breach block
    # Equation: blocked = a validity check failed (fit-config checks.validity_*) | Known: eligible with a reported miss; blocked; blocked

  Scenario: M2 predicted RAM from equation 4 at the target size, else pending
    Given a predicted run list cell at a size with no run
    Then its RAM is pending, never another size's M_cctx
    # Equation: M_comp(target) = M_0 + M_cctx(L, W_eff(px)) + B_c * px * 3 (+ M_train) | Known: pending

  Scenario: #15 a non-text row passes O only where every text scenario passes at its q
    Given text scenarios with R = 1 at q 50 and typing R = 0.9 at q 60
    Then flash at q 50 is kept and flash at q 60 is dropped, whatever q_min is
    # Equation: O(non-text, E, q) = every text scenario at the same E, size and q has R = 1 | Known: kept, dropped

  Scenario: M7 PSNR is inf at MSE 0
    Given D rows that report PSNR inf and MSE 0
    Then the configuration's PSNR is inf and every quality coefficient is finite
    # Equation: PSNR = 10 * log10(255^2 / MSE), inf at MSE = 0 | Known: inf; 0 coefficients not finite

  Scenario: #16 held-out rows never change a coefficient
    Given the input CSVs, fitted twice: once as they are, once with every held-out row's measurements changed
    Then every equation (coefficients, residuals, q_min) is the same
    # Equation: coefficients = f(training rows only) | Known: identical

  Scenario: #17 size groups follow the run list cell, not the resolved window
    Given ZW12 (prefix, window resolved from the frame size) at 640x360 and 1280x720 in the fit and 1920x1080 held out
    Then the held-out bytes are predicted from the fitted intercept
    # Equation: zeta(px) = zeta(px0) * (px / px0)^(eta - 1), one group per run list cell | Known: exact within 1e-6

  Scenario: #18 M5 D bytes: P and K apart, predicted only where a GOP sweep supports it
    Given D configurations made exactly from P = p * px * exp(a q) and K = k * px * exp(b q) with b != a, hint-off rows, and GOP sweeps at q 70 only, then at q 50 and 70
    Then with one sweep q 70 tau_f 100 is exact and q 50 tau_f 100 is pending; with two sweeps a_E and b_E are fitted apart and q 90 tau_f 200 is exact
    # Equation: bytes = (n - n_key) * P + n_key * K, n_key = ceil(n / tau_f) | Known: within 1e-6; pending where no sweep supports

  Scenario: eq 4 fitted terms: mu and m_d0 from training rows
    Given ZW16 rows made with M_train = mu * S_samples and decoder rows with a fixed overhead m_d0, ZW16 1920x1080 held out
    Then mu and m_d0 are fitted back from the training rows, and the held-out RAM and decoder RAM are predicted with them
    # Equation: M_comp = M_0 + M_cctx + B_c * px * 3 + mu * N_train * px * 3; M_dec = M_0 + M_dctx + B_d * px * 3 + b_max + m_d0 | Known: mu 1.9, m_d0 800000 B, within 1e-6

  Scenario: eq 4 fitted terms on the input CSVs
    Given the Z input CSVs of fit.json
    Then mu is fitted from the ZW16 training rows (pending without one) and m_d0 from the decoder training rows, with residuals
    # Equation: mu = sum(S * e) / sum(S^2), e = ram_os - (known_m_comp - M_0); m_d0 = mean(dec_peak_private - known_m_dec) | Known: finite coefficients; residuals listed

  Scenario: #20 predicted cells appear with status and uncertainty
    Given a run list cell marked predicted
    Then the fit has a row for it per content with its prediction status, predicted values and uncertainty
    # Equation: rows(predicted) = predicted cells * contents | Known: equal; bytes/frame exact within 1e-6

  Scenario: fitter self-test: power law in px
    Given data made exactly from ln zeta = ln zeta_0(g) + (eta - 1) * ln(px / px0) with known zeta_0 and eta
    Then the fitter returns those coefficients
    # Equation: zeta(px) = zeta_0 * (px / px0)^(eta - 1), shared eta, one zeta_0 per group | Known: every coefficient within 1e-6 relative

  Scenario: fitter self-test: px-linear CPU
    Given data made exactly from y = c_0 + kappa(g) * px with known c_0 and kappa
    Then the fitter returns those coefficients
    # Equation: C_comp / N_new = c_0(m) + px * kappa_c(L, m, c) | Known: every coefficient within 1e-6 relative

  Scenario: fitter self-test: D curves
    Given data made exactly from P = p * px * exp(a * q), Q_ssim = 1 - s * exp(-g * q), Q_psnr = m + l * q, c_enc = e0 + A * px + B * px * q, M_enc = m0 + m1 * px
    Then the fitter returns those coefficients
    # Equation: equation 7 forms | Known: every coefficient within 1e-6 relative

  Scenario: fit file
    Given results/fit.json written by fit/fit.mjs
    Then it names its input CSVs with their row counts and is labeled PILOT when an input is a pilot CSV
    # Equation: rows(fit.json inputs) = rows(CSV) | Known: equal for every input CSV

  Scenario: eq 1 Z bytes
    Given the Z configurations in the fit set
    Then zeta(L, m, c, px) is given for each, and eta per mode with residuals where a group runs at two sizes or more
    # Equation: b_k = zeta * px * 3 (+ h); zeta(px) = zeta(px0) * (px / px0)^(eta - 1) | Known: coefficients and residuals listed; eta pending while every group has one size

  Scenario: eq 2 Z quality
    Given every Z run
    Then the round trip is exact
    # Equation: sum |decoded - corpus| = 0  =>  SSIM-Y = 1, PSNR-Y = inf, R = 1 | Known: diff_sum = 0 on every run

  Scenario: eq 3 Z CPU
    Given the Z configurations in the fit set
    Then kappa_c(L, m, c) and c_0(m) are fitted with residuals, and kappa_d(L, m) is given
    # Equation: C_comp = N_new * (c_0(m) + px * kappa_c(L, m, c)); C_dec = N_dec * px * kappa_d(L, m) | Known: coefficients and residuals listed; c_0 pending while every group has one size

  Scenario: eq 4 Z RAM
    Given every Z run with its equation-4 value
    Then the measured peak private bytes are carried with their residual against the equation
    # Equation: M_comp = M_0 + M_cctx(L, W) + B_c * px * 3; M_dec = M_0 + M_dctx(W) + B_d * px * 3 | Known: residuals listed; share within 10 % reported

  Scenario: eq 5 Z time until ready
    Given every Z run
    Then t_ready is at most one instant
    # Equation: t_ready = t_close | Known: at most 50 ms on every run

  Scenario: eq 6 run time
    Given the Z run list and a complete Z run
    Then the run's wall time is within 20 % of T_total of the run list
    # Equation: T = sum over runs of N_new * px * 3 / v(L, m) * repeats | Known: within 20 % (pending until the Z CSV covers the run list)

  Scenario: eq 7 D bytes
    Given the D configurations in the fit set at tau_f 20
    Then p_E,c and a_E are fitted with residuals; P and K are split where tau_f varies
    # Equation: bytes_D = t * (n_new * P + (n_new / tau_f) * (K - P)); P = p_E,c * px * exp(a_E * q) | Known: coefficients and residuals listed; K/P pending without a tau_f sweep

  Scenario: eq 7 D quality
    Given the D configurations in the fit set
    Then s_E,c, g_E, m_E,c and l_E are fitted with residuals
    # Equation: Q_ssim = 1 - s_E,c * exp(-g_E * q); Q_psnr = m_E,c + l_E * q | Known: coefficients and residuals listed

  Scenario: eq 7 D q_min
    Given every D configuration of a text scenario
    Then q_min(E, px) is the lowest q with R = 1 on every text scenario, or "not reached"
    # Equation: q_min(E, px) = min { q : R = 1 on typing, scroll and ui } | Known: a value or "not reached" for every (E, px) with runs

  Scenario: eq 7 D CPU
    Given the D configurations in the fit set
    Then c_comp, c_file and c_enc(E) = e0_E + e1_E * px * (1 + e2_E * q) are fitted with residuals
    # Equation: a_rt = n_new * px * c_comp(-3) + n_new * c_file; a_w = n_new * px * (c_dec(-3) + c_conv) + n_new * c_enc(E, q, px) | Known: coefficients and residuals listed; pending while every run has one px

  Scenario: eq 7 D RAM
    Given the D configurations in the fit set
    Then M_live and M_enc = m0_E + m1_E * px are fitted with residuals
    # Equation: M_D = M_live + M_worker; M_worker = M0_w + px * (3 + 1.5) * d + M_enc(E, px) | Known: coefficients and residuals listed; pending while every run has one px

  Scenario: Z validation
    Given the Z cells held out in results/z-runlist.json
    Then each held-out configuration is outside the fit set and is predicted: within 10 % (bytes, CPU, RAM), else "not predictable" with its spread
    # Equation: |predicted - measured| / measured <= 0.10 | Known: every held-out row has a verdict; never refitted

  Scenario: D validation
    Given 20 % of the D configurations drawn by mulberry32 seed 20261122
    Then the draw is reproduced, each one is outside the fit set and is predicted: within 10 % (bytes, CPU, RAM), 0.005 SSIM, else "not predictable" with its spread
    # Equation: |predicted - measured| / measured <= 0.10; |SSIM_pred - SSIM| <= 0.005 | Known: every held-out row has a verdict; never refitted
