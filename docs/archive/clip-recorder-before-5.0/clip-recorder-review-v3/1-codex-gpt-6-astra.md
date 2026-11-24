## A. Coverage table

Reviewed repository HEAD `30392a4694da7fb751330c97a792b9e038b3b61f`, branch `feature/clip-capture`.

**Coverage limitation:** all 18 bundle documents read fully. The large shared facade modules were read partly, including their clip-related sections. This therefore does **not** satisfy an exhaustive whole-file reading of every named source file.

Bundle paths below refer to the supplied `clip-bundle/` directory.

| File | Coverage |
|---|---|
| `00-INDEX.md` | read fully |
| `01-plan-v1-ad0047d1.md` | read fully |
| `02-plan-v1.1-692f6137.md` | read fully |
| `03-plan-v2-f354496d.md` | read fully |
| `04-plan-v3-30392a46.md` | read fully |
| `10-equation-draft-692f6137.md` | read fully |
| `11-equation-9ba1754a.md` | read fully |
| `12-equation-base-rules-b019b8a0.md` | read fully |
| `13-equation-approved-weakest-machine-8fceb267.md` | read fully |
| `20-issue-290-work-log-and-reviews.md` | read fully — all three comments |
| `21-issues-285-286-gif-recorder.md` | read fully |
| `30-research-full-results.md` | read fully |
| `31-research-synthesis-report.md` | read fully |
| `32-note-bug_clip_recording_patterns.md` | read fully |
| `32-note-frame_selection_for_image_agents.md` | read fully |
| `32-note-video_and_film_fundamentals_academic.md` | read fully |
| `32-note-video_capable_ai_models.md` | read fully |
| `32-note-windows_capture_and_encoding.md` | read fully |

Repository paths relative to `C:/Users/Adam/Synapse-clip/`:

| File | Coverage | F1–F9 check where applicable |
|---|---|---|
| `crates/synapse-mcp/src/server/capture_clip.rs` | read fully | F4: WGC pinned; no recorder backend fallback |
| `crates/synapse-mcp/src/server/clip_encoder.rs` | read fully | F8: hardware requested, identification best effort; can return `unknown`. F9: fragmented-MP4 attempt and plain-MP4 fallback present; crash guarantees not established |
| `crates/synapse-mcp/src/server/clip_decoder.rs` | read fully | |
| `crates/synapse-mcp/src/server/clip_viewer.rs` | read fully | |
| `crates/synapse-mcp/src/server/m1_tools/mod.rs` | read partly | Clip routing, validation, target resolution and surrounding sections |
| `crates/synapse-mcp/src/server/tool_profiles.rs` | read partly | Screenshot contracts and additional facade sections |
| `crates/synapse-mcp/src/desktop_worker.rs` | read fully | F6 true: self-launch and kill-on-close job exist; launcher also imposes a 10-second timeout |
| `crates/synapse-capture/src/platform/windows/capture.rs` | read fully | F1 true: per-frame staging/readback. F2 true at line 279. F4 settings present at lines 129–149 |
| `crates/synapse-capture/src/frame.rs` | read fully | |
| `crates/synapse-storage/src/pressure.rs` | read fully | F7 true: 30-second polling, Level 1 below 2,000,000,000 bytes; actual constants at lines 15 and 18 |
| `crates/synapse-mcp/src/server/capture_gif.rs` | read fully | #285 synchronous capture inside async function confirmed; #286 browser-tab-to-window mapping confirmed |
| `crates/synapse-mcp/src/m1.rs` | read partly | Screenshot/clip enums, parameters, limits and responses |
| `crates/synapse-mcp/src/m3/timeline_control.rs` | read fully | Pause/exclusion semantics |
| `scripts/clip-equation/bench.rs` | read fully | |
| `scripts/clip-equation/measure_encoder.ps1` | read fully | |
| `scripts/clip-equation/run_equation.py` | read fully | |
| `scripts/clip-equation/flash-page.html` | not read | Missing; proposed proof artifact |
| `README.md` | read partly | Clip/tool-surface entries |
| `docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md` | read partly | Screenshot operation table |

Dependency paths relative to `C:/Users/Adam/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows-capture-2.0.0/`:

| File | Coverage | Fact check |
|---|---|---|
| `src/graphics_capture_api.rs` | read fully | F3: one-buffer pool true. Its claimed no-loss consequence is unsupported. F4: unsupported-setting errors true; cursor settings require gating too |
| `src/frame.rs` | read fully | F5 true: `timestamp()` returns `SystemRelativeTime()` |

Windows feasibility additionally checked against Microsoft documentation for [job CPU caps](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information), [H.264 encoding](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder), [forced keyframes](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avencvideoforcekeyframe), [MP4 sinks](https://learn.microsoft.com/en-us/windows/win32/medfound/mpeg-4-file-sink), and [D3D11 context threading](https://learn.microsoft.com/en-us/windows/win32/direct3d11/overviews-direct3d-11-devices-intro).

No builds, cargo, application operation or commits. Tool caveat: Serena activation reported project-metadata creation; git shows untracked `.serena/`. No cleanup attempted under the read-only instruction.

## B. Findings

`VERIFIED` means the cited text/code establishes the defect or contradiction; it does not mean runtime reproduction.

- `04-plan-v3-30392a46.md:149 | blocker | Explicitly accepts frame loss on a slow disk, contradicting C and BR4; reporting failure does not make V=1. The approved equation itself exposes this unresolved conflict at 13-equation-approved-weakest-machine-8fceb267.md:250 | Resolve the live storage requirement before implementation: demonstrate a lossless affordable representation, or obtain an explicit narrower guarantee | VERIFIED`
- `04-plan-v3-30392a46.md:137 | blocker | Fixed 4% worker plus assumed 1% live share cannot enforce BR2. Under the published i3 cost model, 20 new 4K frames/sec require approximately 2.1% for readback alone, giving 6.1% with the worker before observation/disk costs | Bound and measure the complete live cost; allocate only the remaining aggregate CPU budget to deferred work | VERIFIED`
- `scripts/clip-equation/bench.rs:72 | major | “Readback” measurement is RAM copy_from_slice, not GPU staging/copy/Map. run_equation.py:14 uses it as c_read; observer and disk costs are absent | Replace the readback estimate with measurements of the proposed capture-to-spool path; retain the i3 figures as estimates until measured there | VERIFIED`
- `04-plan-v3-30392a46.md:62 | blocker | Handler duration below T_r does not prove C: Windows delivery, scheduling, frame acquisition and pool recreation occur outside the measured handler. Research already reports approximately 45 fps for a 16.66 ms request | Remove the sufficient-condition claim; distinguish proven loss, unknown delivery completeness and measured controlled-case coverage | VERIFIED`
- `04-plan-v3-30392a46.md:49 | blocker | Four textures assume bounded arrival rate and zero selection delay. A stalled consumer can lose the required frame while every callback remains short; Win10 and cursor activity can exceed 60 deliveries/sec | Specify delivery watermark, maximum selection lateness, pinned-slot ownership, overflow handling and ring capacity derived from those bounds | VERIFIED`
- `04-plan-v3-30392a46.md:45 | major | GPU-ring/readback threading and completion ownership are unspecified. D3D11 contexts cannot be used concurrently; a shared lock held across blocking Map can stall capture | Define context ownership, ordered copies, staging completion and non-overwrite rules before implementing O2/O4 | VERIFIED`
- `04-plan-v3-30392a46.md:47 | major | O4 performs readback before A6 checks whether the frame repeats, contradicting “a repeated frame costs no readback” | Return/select frame identity first; perform readback only after the repeat check | VERIFIED`
- `04-plan-v3-30392a46.md:44 | major | Win10 fallback omits cursor-setting support. The crate rejects explicit WithCursor where unsupported; its base support check also requires UniversalApiContract 8 | State the minimum supported Windows 10 build and gate cursor settings alongside border, interval, secondary-window and dirty-region settings | VERIFIED`
- `04-plan-v3-30392a46.md:63 | major | Resize settling reintroduces letterboxing that the approved equation superseded; continuous dragging can violate G indefinitely. CPU letterboxing also contradicts A7’s “only one copy” claim | Preserve native-sized captured frames and handle resizing in deferred work, or explicitly amend G and account for live scaling | VERIFIED`
- `04-plan-v3-30392a46.md:116 | major | Removing Q’s verdict contradicts V=C·G·Q·B·T and the approved per-condition reporting rule. q=70 and β derived from average output size do not establish readability across encoders/resolutions | Calibrate q_min and fallback readability for supported paths; report Q as established, failed or unverified against that calibration | VERIFIED`
- `04-plan-v3-30392a46.md:74 | major | Media Foundation selection establishes neither “cheapest encoder” nor the equation’s minimum-byte E(w,h). Only size refusal receives an explicit software retry; other hardware failures are unresolved | Keep H.264, but define supported-path selection and bounded software replay from retained inputs; qualify the optimization claim | VERIFIED`
- `04-plan-v3-30392a46.md:75 | major | Timestamps alone do not specify VFR display duration, the final hold, or segment-local timing. “No encoder work after the first frame” conflicts with independently playable 60-second static segments | Specify sample durations through stop, global-to-segment timestamp mapping and an initial sample for each independently playable segment | VERIFIED`
- `04-plan-v3-30392a46.md:76 | major | Force-keyframe applies to the next MFT ProcessInput, not necessarily the intended Sink Writer submission when buffering intervenes; requested GOP positions are not verified output keyframes | Validate keyframe placement on software/Intel paths and derive the index from actual output; assess fallback by decode distance and measured seek time | CANDIDATE`
- `04-plan-v3-30392a46.md:81 | major | Deleting raw input when handed to the encoder precedes durable output. Fragment buffering and fragment duration are unspecified; removing τ’s crash constraint does not establish a replacement durability bound | Retain inputs until an independently recoverable segment is finalized and durably committed, then delete them | VERIFIED`
- `04-plan-v3-30392a46.md:79 | major | Restart scan misses “last raw file deleted, clip.json not written.” No durable initial descriptor, segment commit protocol or custom-folder discovery is specified; finished-segment viewing also needs metadata before final clip.json | Persist initial clip state and atomic segment checkpoints; recover all incomplete states idempotently, including empty waiting/ | VERIFIED`
- `crates/synapse-mcp/src/desktop_worker.rs:1598 | major | Existing launcher waits synchronously, times out after 10 seconds and requires an HWND; deferred encoding can take minutes and outlive the window | Reuse job/process ownership code, but add a clip-specific lifetime, arguments and progress contract without the capture timeout/HWND dependency | VERIFIED`
- `04-plan-v3-30392a46.md:56 | major | One active recording does not imply one encoding worker: prior clips and restart recovery can overlap, each receiving a separate 4% job | Specify one global encoding queue or one shared aggregate capped job, including recovered work | VERIFIED`
- `04-plan-v3-30392a46.md:55 | major | Start gating and “stop when privacy pauses” do not preserve the explicit frame-zero/per-frame write gate from #290. Existing monitor privacy checks only the foreground app; visible excluded background windows remain unaddressed | Define privacy checks before persistence, queue cutoff on pause/exclusion, and monitor-visible exclusion semantics | VERIFIED`
- `04-plan-v3-30392a46.md:64 | major | A once-per-second free-space check cannot guarantee “disk never fills”; output-volume selection, raw backlog, concurrent writers and finalization failures remain unspecified | Probe the actual spool/output volume, account for pending writes and finalization, and define recoverable disk-full behavior | VERIFIED`
- `04-plan-v3-30392a46.md:88 | major | States recording/encoding/ready cannot represent worker failure, interrupted capture, failed recovery or disk-full termination. Session ownership and cancellation semantics changed without a replacement contract | Define terminal failure/interruption states, ownership transfer at successful start, idempotent stop and retention of already committed segments | VERIFIED`
- `crates/synapse-mcp/src/server/clip_viewer.rs:593 | major | Overview chooses transient onset only when the whole episode reverts. A 50 ms local flash during continuous animation can be omitted; cursor masking promised in v1 remains explicitly absent at line 637 | Validate transient selection during unrelated motion; preserve local transient candidates and implement the promised cursor/noise handling | VERIFIED`
- `04-plan-v3-30392a46.md:143 | major | Fixed eight images conflicts with N derived from token budget. Eight 75%-scale 4K images cost approximately 50,000 tokens under the approved formula; the existing viewer repeats the entire timeline on every page | Page images and timeline by measured response budget while preserving readable scale; update public image-size limits accordingly | VERIFIED`
- `04-plan-v3-30392a46.md:171 | major | Two 30-second development-PC clips cannot exercise 60-second boundaries, long static holds, slow storage, concurrent recovery, failure paths or Win10/i3 behavior. Merely opening the viewer does not prove the agent sees the flash | Make those manual Synapse cases and real-agent readability checks explicit release gates; sweep flash phase and verify presented timing | VERIFIED`
- `04-plan-v3-30392a46.md:134 | minor | 4096×2304 is presented as a universal H.264 level-5.1 width/height limit; level constraints and individual MFT acceptance are different | Name it as a chosen implementation bound and validate actual encoder acceptance, including portrait sizes | VERIFIED`
- `04-plan-v3-30392a46.md:127 | minor | T_r means seconds per refresh in the equation but Hz in the parameter map; the ring formula consequently changes the symbol’s units | Use separate refresh-frequency and refresh-period symbols with explicit unit conversion | VERIFIED`

## C. Lost or dropped items from v1 to v3

The justified trajectory: **v1 → v1.1** established equation precedence. **Equation 10 → 11** corrected the 144 Hz flash derivation and added machine-independent CPU requirements. **12** replaced adaptive recording rates with the owner’s fixed 20 fps/60 Hz/5% rules. **13** added explicitly unmeasured i3 estimates. **V2** translated those requirements into asynchronous recording, deferred encoding, VFR, minimum-size duplication and resize segments. **V3** correctly added the capture-layer work, timestamp correction, OS capability checks, existing-worker reuse, shared storage threshold and corrected overview scaling.

H.264/Media Foundation and image-only viewing remain justified. VFR is a reasonable response to measured repeat-encoding cost. The 5-second keyframe proposal has measured size-saving evidence, but not completed seek/crash validation. Ring size four, 500 ms settling, the 4%/1% split and encoder-size bound remain assumptions. Reusing worker infrastructure and pressure thresholds reduces duplication; clip-specific observation is justified by F1–F5. GIF unification remains an explicit deferral, not a missing prerequisite.

- **“Only time gives way” weakened:** v2/v3 permit slow-disk capture loss; last uncompromised requirement: `13-equation-approved-weakest-machine-8fceb267.md:38`, repeated but contradicted in v3.
- **Per-refresh observation evidence replaced by callback timing:** last required explicitly in `13-equation-approved-weakest-machine-8fceb267.md:69`; #290 comment 2 introduced the invalid substitute.
- **Immediate native-size resize behavior weakened:** last present in `03-plan-v2-f354496d.md:33`; v3 adds potentially indefinite letterboxing.
- **Q pass/fail deliberately removed:** last present in `03-plan-v2-f354496d.md:77`; #290’s “q_min measured once” argument does not justify dropping conformance reporting.
- **Crash bound on τ removed without durable-input replacement:** last present in `03-plan-v2-f354496d.md:94` and approved equation line 138; #290’s audit overlooked deletion of already-submitted raw frames.
- **No-child-process owner decision unresolved:** last fixed statement in `02-plan-v1.1-692f6137.md:30`; v3 correctly lists the separate process as still awaiting a decision, so it must not be treated as approved.
- **Explicit frame-zero/per-frame privacy ordering absent from the new process:** last reaffirmed in `20-issue-290-work-log-and-reviews.md:107`; current code retains it.
- **Minimized-window intervals and never-focus/restore behavior no longer explicit:** last detailed in `02-plan-v1.1-692f6137.md:66`; current code still records minimized intervals.
- **All-black/backend-failure behavior lacks an explicit carry-forward rule:** last detailed in `02-plan-v1.1-692f6137.md:74` and `20-issue-290-work-log-and-reviews.md:36`.
- **Cursor mask promised but never delivered:** last plan requirement in `02-plan-v1.1-692f6137.md:93`; current viewer explicitly says it has no mask.
- **Caret suppression and transient recall under unrelated animation remain unaddressed:** research requirements in `32-note-frame_selection_for_image_agents.md:39`; v3’s proof checks capture, not selection effectiveness.
- **Real-agent h_leg calibration missing from v3 proof:** last explicit measurement in `13-equation-approved-weakest-machine-8fceb267.md:165`; decoded-text inspection alone is insufficient.
- **N derived from token budget reverted to a built constant:** last explicit requirement in `13-equation-approved-weakest-machine-8fceb267.md:189`.
- **Monitor overlap privacy remains unimplemented:** last explicit research rule in `30-research-full-results.md:202`.
- **Earlier ten code-review fixes lack explicit regression acceptance in the redesign:** last consolidated in `20-issue-290-work-log-and-reviews.md:34`; especially cancellation, backend failure, startup timing, cache invalidation, final-frame intervals and encoder finalization. This is missing carry-forward evidence, not proof those fixes were already reverted.
- **Rolling capture, triggers, events, retention, local/video models and GIF unification:** explicitly excluded or deferred by owner/scope decisions; not silent losses.

## D. CONCLUSION

V3 chooses viable Windows building blocks, but its claimed guarantees do not follow from them. BR1’s 20 fps recording schedule is sound; **C** lacks a valid delivery/retention proof, **G** is knowingly violated during resizing and oversize capture, **Q** remains uncalibrated, **B/BR2** lack a complete aggregate CPU bound, and **T** conflicts with destructive pre-commit deletion and incomplete recovery. **BR4 cannot hold for the admitted slow-disk case.** WGC timestamps, a GPU texture ring, VFR Media Foundation encoding and a capped job are technically feasible; their synchronization, timing, durability and lifecycle contracts need correction. The existing research supports this architecture’s direction, not its universal guarantees. Resolve those blockers and validate the live path on Windows 10/i3 before committing to the full build; the coverage limitations above do not weaken the direct contradictions already established.

VERDICT: not ready.