> **OPINION, NOT PROOF.** Answer of Codex (gpt-6.1-sol, high effort) to the bare task (file `question.md`), given in an empty folder on 2026-11-23, from the model's own knowledge. Nothing here is a finding until it is proven from a source in `../proof.md`.

**My default choice: capture the window with Windows Graphics Capture, encode on the GPU, and save a timestamped visual record. Add UI Automation text and occasional lossless screenshots when the model needs exact text.** Try hardware AV1 and HEVC first; keep hardware H.264 as the compatibility option.

There is no unconditional winner across all four goals. Lossless pixels, minimum file size, minimum CPU, and minimum RAM pull in different directions. The application, GPU, and model’s input pipeline determine the best compromise.

This is from my own knowledge. No files, tools, or current product documentation consulted. Product details I cannot confidently establish are marked **unsure**.

## 1. The choices are separate layers

A complete solution chooses:

1. **Capture:** where the window’s pixels or state come from.
2. **Representation:** video, screenshots, changed rectangles, text, events, drawing commands, or execution history.
3. **Encoding:** how that representation is compressed.
4. **Recording policy:** when to sample, what to retain, and whether to encode locally or elsewhere.
5. **Playback for the model:** how to recover images, text, timing, and actions.

A codec alone does not solve window capture. An event log alone does not establish what appeared on screen.

For the tables below:

- **Quality** means preserving visible content, readable text, transitions, and timing.
- **CPU/RAM** refer primarily to the recording PC. GPU memory still counts as a resource cost.
- **Build cost:** low means assembling existing tools; medium means a maintained custom recorder; high means substantial integration, compatibility, or application-specific work. These are relative estimates, not quotations.

## 2. Fundamentally different capture methods

| Method and concrete names | Quality | File size | CPU | RAM | Failure points; build cost |
|---|---|---|---|---|---|
| **Windows Graphics Capture:** `Windows.Graphics.Capture`, `IGraphicsCaptureItemInterop::CreateForWindow`, D3D11 frame pool | Excellent source for an individual window; ordinarily handles occlusion | Determined downstream | Low with GPU processing | Several GPU surfaces; bounded with a small pool | Minimized, protected, closing, and unusual windows need testing. Popups belonging to other HWNDs may need separate capture. **Medium** custom; **low** through OBS |
| **Desktop Duplication:** `IDXGIOutputDuplication`, dirty rectangles, move rectangles | Excellent monitor pixels | Dirty/move metadata can support efficient recording | Low with GPU path | Monitor-sized GPU surfaces | Cropping a window from the desktop captures overlapping windows and loses content when minimized. **Medium** |
| **GDI screen capture:** `BitBlt`, `GetDC`, FFmpeg `gdigrab` | Good for ordinary visible desktop content | Determined downstream | Often higher due to readback and copies | CPU pixel buffers | Occlusion, DPI, accelerated content, and capture rate can cause problems. **Low** |
| **Ask the application to paint:** `PrintWindow`, `WM_PRINT`, `WM_PRINTCLIENT` | Can capture content without reading the visible desktop | Determined downstream | Variable; can be expensive | Usually CPU bitmaps | Application cooperation varies; blank, incomplete, stale, or blocking results. **Low–medium** |
| **Hook the rendering API:** Direct3D/OpenGL/Vulkan presentation interception; OBS Game Capture | Excellent rendered frames, often with little copying | Determined downstream | Low after setup | GPU surfaces | Injection, anti-cheat, sandboxing, API differences; may omit native dialogs or overlays. **High** from scratch; **low** where OBS works |
| **Capture inside the application:** export a render target, canvas, document, or browser surface | Potentially best source and most precise timing | Can export compact native state as well as pixels | Potentially very low | Potentially low | Requires application integration; may omit OS chrome and external dialogs. **Medium–high** |
| **Virtual or remote display:** RDP session, virtual monitor, WDDM indirect display driver/`IddCx` | Consistent controlled display | Can exploit remote-display compression | Low–medium; implementation dependent | Display and codec buffers | Changes the environment being recorded; may affect acceleration, resolution, session behavior, and app compatibility. **High** |
| **Browser-specific capture:** Chromium tab capture, `getDisplayMedia`, DevTools screencast | Good for browser content | Determined by capture/encoding policy | Low–medium | Browser plus capture buffers | Browser-only; permissions, throttling, capture limits, and UI outside the page. DevTools screencast is not a universal high-fidelity recorder. **Low–medium** |
| **External digital capture:** HDMI/DisplayPort output into a recorder or second PC | Excellent display output within signal/codec limits | Depends on external encoder | Very low additional host CPU | Very low additional host RAM | Captures a display, not an isolated hidden window. Requires display arrangement and hardware. **Low software effort; hardware cost** |
| **Optical recording:** camera pointed at the screen | Records what a person could see | Usually larger for equivalent text readability | Almost none on host | Almost none on host | Moiré, glare, exposure, refresh artifacts, and poor small-text fidelity. **Low integration; poor fit** |

**Important:** ordinary screenshots, image libraries, and encoding libraries do not themselves solve reliable window capture. Pillow, OpenCV, and FFmpeg still need a suitable capture source.

Also, a DWM thumbnail is a compositor preview facility, not a general-purpose recording API.

## 3. Fundamentally different representations and codecs

### Continuous or frame-based visual records

| Representation / codecs | Quality | File size | CPU | RAM | Failure points; build cost |
|---|---|---|---|---|---|
| **Raw RGB/BGRA frames** | Exact source pixels | Enormous | Low encoding CPU; heavy memory/disk traffic | Bounded if streamed, enormous if accumulated | Storage bandwidth dominates. **Low** |
| **Raw or lightly compressed YUV frames** | Depends on conversion; 4:2:0 loses color detail | Very large | Low | Bounded streaming buffers | Tiny colored text can degrade before encoding starts. **Low** |
| **H.264/AVC** | Very good at suitable settings | Small | Low with hardware; software varies widely | Usually modest with bounded queues | Thin text and motion can suffer at low bitrate; 4:2:0 is common. Excellent compatibility. **Low** |
| **HEVC/H.265** | Very good | Often smaller than H.264 at comparable visual quality | Low with hardware; software can be costly | Hardware modest; software depends on settings | Decoder support, licensing, and screen-content performance vary. **Low–medium** |
| **AV1** | Very good | Often highly efficient | Low CPU with supported hardware; software can be expensive | Depends on encoder buffers and settings | Hardware availability, decoding cost, and exact screen-content performance. **Low–medium** |
| **VP8 / VP9** | Good to very good | VP9 generally more efficient than VP8 | Software cost can be significant; hardware support varies | Moderate, setting dependent | Often less convenient than H.264/HEVC/AV1 for Windows hardware recording. **Low–medium** |
| **Older interframe codecs:** MPEG-1/2, MPEG-4 Part 2/ASP, H.263, WMV/VC-1, Theora | Usable, generally inefficient by current standards | Larger for equivalent quality | Sometimes cheap to encode | Often modest | Weak reason to choose them for a new system. **Low** with existing libraries |
| **Newer or specialized standards:** VVC/H.266, EVC, AVS families | Potentially excellent compression | Potentially small | Practical encoder support is the problem | Implementation dependent | Windows availability, licensing, decoding, and hardware support. Current product readiness **unsure**. **High** |
| **Enhancement-layer coding:** LCEVC over a base codec; scalable video coding such as H.264 SVC or scalable AV1 | Multiple quality/resolution levels | Can reduce duplicated delivery streams | Additional processing and integration | Additional buffers | More useful for distribution than a single local archive. Deployment support **unsure**. **High** |
| **Independent JPEG frames / MJPEG** | Good at high quality; artifacts around text | Large | Relatively low; parallelizable | Small bounded buffers | No temporal compression. **Low** |
| **Production intraframe codecs:** ProRes, DNxHD/DNxHR, CineForm | High quality; easy frame access | Large | Usually less demanding than slow interframe compression, but not free | Moderate | Designed for editing, not minimum storage. **Low–medium** |
| **Wavelet/image-oriented video:** JPEG 2000, Dirac/VC-2 | High quality; some lossless options | Usually larger than efficient interframe delivery codecs | Variable, often substantial | Variable | Limited practical benefit here. **Medium** |
| **Lossless video:** FFV1, HuffYUV, Lagarith, Ut Video | Exact pixels if the input format is preserved | Small on some static screens; large with motion/noise | Variable; can be substantial | Codec dependent; can be bounded | Noisy video, scrolling, and animation can make files large. **Low–medium** |
| **Lossless predictive or screen codecs:** ZMBV, CamStudio Lossless/CSCD, TechSmith Screen Capture Codec/TSCC; QTRLE | Excellent for appropriate desktop content | Potentially very compact on simple/static UI | Often low–moderate | Usually bounded | Old ecosystems, format limitations, and modern decoder support. TSCC is associated with older Camtasia; current Camtasia internals **unsure**. **Medium** |
| **Lossless modes of general codecs:** `libx264rgb`, lossless x264/x265, lossless VP9/AV1 | Can preserve exact samples in supported formats | Content dependent | Often expensive in software | Settings dependent | “Lossless” after a lossy RGB→YUV conversion does not preserve the original desktop pixels. Hardware support is limited and must be checked. **Medium** |
| **GPU texture video:** Hap, related block-compressed texture approaches; Resolume DXV | Fast GPU playback; high visual quality at suitable settings | Usually large | Encoding varies; decoding can be light | GPU-oriented; bandwidth and VRAM matter | Optimizes playback and effects, not compact archival. Exact formats can be lossy. **Medium** |

Hardware and software implementations of the same codec can differ substantially. “AV1” does not specify one quality, speed, or memory footprint.

### Images, deltas, and sparse visual records

| Representation | Quality | File size | CPU | RAM | Failure points; build cost |
|---|---|---|---|---|---|
| **PNG screenshot sequence** | Exact pixels | Excellent on flat UI; poor on photographs/video | Moderate compression cost | Low if streamed | Full-frame repetition wastes space unless deduplicated. Sparse sampling misses transitions. **Low** |
| **QOI or other fast lossless image sequence** | Exact pixels | Usually larger than PNG | Low compression CPU | Low streaming memory | Trades storage for speed; fewer ready-made playback tools. **Low–medium** |
| **JPEG screenshot sequence** | Good at suitable settings | Small per frame | Low–moderate | Low | Text/color artifacts; no temporal compression. **Low** |
| **WebP / AVIF / HEIF / JPEG XL images** | Lossy or lossless options, depending on format/profile | Potentially smaller images | Encoding can be costly, especially AVIF | Implementation dependent | Tool and model-pipeline support vary. JPEG XL support in the intended playback chain **unsure**. **Medium** |
| **Animated GIF** | Poor for general desktop fidelity: palette limits | Small only for simple animation | Can be expensive to optimize | Tools may retain many frames | Bad fit for small colored text, gradients, and long clips. **Low** |
| **APNG / animated WebP / animated AVIF** | Lossless or high quality depending on format | Better than repeated full screenshots in some cases | Variable | Some implementations buffer heavily | Seeking, duration, compatibility, and temporal efficiency often inferior to video. Animated AVIF support **unsure**. **Low–medium** |
| **Changed rectangles or tiles** | Exact with lossless tile encoding | Excellent when little changes | Low–moderate; hashing/comparison costs | Previous frame or tile state | Scrolling can dirty much of the window. Need reconstruction and recovery points. **Medium** |
| **XOR frame deltas + compression:** zstd, LZ4, DEFLATE | Exact | Excellent for some static UI | LZ4 favors speed; stronger compression costs more | At least prior/current state or equivalent | Motion, anti-aliasing changes, and noise weaken compression. **Medium** |
| **Move/copy operations + new pixels** | Exact if correctly detected | Especially good for scrolling and moved content | Detection adds work | Previous reconstructed image | Incorrect move detection corrupts replay; source metadata helps. **Medium–high** |
| **Content-addressed tile/frame storage** | Exact | Excellent for repeated views and duplicates | Hashing and bookkeeping | Cache can grow unless bounded | Few gains on unique motion; must manage indexes and retention. **Medium–high** |
| **Adaptive screenshots:** periodic, on change, after input, or after visual settling | Excellent captured frames | Very small for quiet workflows | Low–moderate | Low | Misses brief errors, animation, loading states, and causal order between samples. **Medium** |
| **ROI/foveated recording:** preserve text or active regions more strongly | High where correctly prioritized | Smaller than uniform high quality | Region detection adds cost | Usually modest additional state | Missed regions lose evidence. Encoder ROI support varies. **Medium–high** |
| **Multiresolution record:** small overview + full-resolution crops or keyframes | Good overall context plus exact detail | Can be compact | Multiple processing paths | Extra surfaces/buffers | More complicated reconstruction and model ingestion. **Medium–high** |

### Structural, semantic, and event records

| Representation and concrete names | Quality for later understanding | File size | CPU | RAM | Failure points; build cost |
|---|---|---|---|---|---|
| **Accessibility tree and text:** Windows UI Automation, `IUIAutomation`, MSAA/`IAccessible` | Excellent for exposed text, roles, values, bounds, and state | Tiny compared with video | Usually low if event-driven; full-tree polling can be expensive | Usually low, but huge trees/caches hurt | Custom canvases, games, some Electron/web content, inaccessible controls, and visual styling. **Medium–high** |
| **Input and window events:** Raw Input, hooks, `SetWinEventHook`, focus/title/window lifecycle | Precise actions and timing | Tiny | Low if implemented carefully | Low | A click does not prove its outcome. Programmatic changes happen without input. **Medium** |
| **Application-native events/state:** command log, document revisions, model changes | Potentially the best account of meaning | Tiny to moderate | Usually low | Usually low | Requires integration; may omit visual defects and intermediate rendering. **High** across arbitrary apps |
| **Browser DOM snapshots and mutations:** DOM, accessibility tree, styles, action log | Excellent for web structure and text | Usually much smaller than video; can still grow | Instrumentation can be significant | Snapshot and mutation state can grow | Canvas/WebGL/video, cross-origin details, dynamic resources, exact rendering. **Medium–high** |
| **Drawing/display-command recording:** GDI orders, vector commands, canvas commands, display lists | Potentially exact replay with compact text/vector data | Tiny for simple UI; large for bitmap-heavy content | Capture and replay overhead vary | Caches/resources can grow | Arbitrary modern apps mix APIs, fonts, shaders, and external resources. **Very high** universally |
| **OCR transcript with timestamps and bounds:** Windows OCR, Tesseract, PaddleOCR | Searchable readable text when recognition succeeds | Tiny transcript | Often substantial unless sampled; GPU options exist | Model dependent | Wrong characters, missed text, poor reading order; loses visual evidence unless images retained. **Medium** |
| **Visual descriptions, embeddings, or model-generated summaries** | Useful index, incomplete evidence | Very small | Potentially high inference cost | Model dependent; can be high | Omissions and hallucinations are irreversible if originals are discarded. **Medium–high** |
| **System telemetry:** ETW, application logs, ProcMon, PresentMon | Excellent for selected technical events | Small to very large | Usually low for narrow tracing; verbose tracing costs more | Buffer/configuration dependent | Does not establish visible content. **Low–medium** |
| **Process execution recording:** WinDbg Time Travel Debugging | Deep causal debugging for supported execution | Can be enormous | Potentially substantial slowdown | Substantial overhead possible | Not a general faithful replay of external GPU/display state. **High** for this purpose |
| **VM snapshots/replay or whole-environment state** | Can preserve environment and application state | Large | Potentially significant | High | Deterministic replay of modern Windows plus GPU/external services is difficult. Current general-purpose product coverage **unsure**. **Very high** |
| **Network/resource archives:** HAR, WARC, captured responses, protocol logs | Useful supporting evidence and replay resources | Content dependent | Usually low–moderate | Can stream to disk | Misses local rendering and interactions not represented by traffic. **Medium** |

**For AI, structural records are valuable supplements.** Exact control text and timestamps can save a model from guessing at compressed text. They do not replace visual evidence across arbitrary applications.

## 4. Encoder implementations are another independent choice

| Encoder route | Four-goal result | Failure points; build cost |
|---|---|---|
| **Fixed-function GPU/media engine:** NVIDIA NVENC, Intel Quick Sync/oneVPL, AMD AMF | Usually the strongest route to small files with low CPU. RAM can remain bounded; GPU surfaces consume VRAM/shared memory. Quality depends on generation/settings | Codec, chroma, profile, resolution, and concurrent-session limits vary. GPU use can affect the application. **Low** through OBS; **medium** custom |
| **Windows Media Foundation hardware MFT** | Can provide the same hardware benefits through a Windows abstraction | Must verify which MFT is actually selected; fallback may use software. Vendor-specific tuning can be awkward. **Medium** |
| **CPU software encoders:** x264, x265, SVT-AV1, libaom, libvpx | Broad control, including some fidelity modes unavailable in hardware. Compression can be excellent | Better compression often costs CPU and memory. Fast presets change the tradeoff. **Low** integration; substantial tuning |
| **GPU compute/shader encoder** | Can move work away from CPU and enable custom processing | Uses GPU resources needed by the app; implementation complexity and quality vary. Usually less attractive than an existing media engine. **High** |
| **Dedicated external encoder** | Very low recording overhead on the application PC | Hardware expense, display capture constraints, and sometimes poorer text settings. **Low–medium** integration |
| **Remote encoding** | Low local encoding CPU/RAM if the transport is efficient | Raw upload bandwidth can be enormous. Compressing before transport already requires local encoding. Adds another machine and failure path. **High** |

“Zero-copy” should mean no unnecessary **CPU readback**, not literally no copies anywhere. Capture, resizing, color conversion, and encoding may still use GPU copies and intermediate surfaces.

## 5. Streaming and remote-display techniques

These can be recorded, but streaming itself does not make a local archive smaller.

| Technique | Four-goal result | Failure points; build cost |
|---|---|---|
| **RDP graphics orders, glyph/bitmap caches, and bitmap updates** | Excellent efficiency for suitable desktop content; can preserve text sharply with low traffic | Behavior depends on negotiated graphics path. Recording requires protocol-aware reconstruction. **High** custom |
| **RDP H.264/AVC and AVC444** | Good interactive graphics; AVC444 addresses desktop color/detail fidelity | Exact negotiation, hardware acceleration, server/client policy, and app behavior matter. **Medium–high** |
| **VNC/RFB rectangles:** Raw, CopyRect, Hextile, ZRLE, Tight; JPEG where supported | Lossless rectangles can be excellent for static UI, with small files and moderate CPU | Full-motion content is expensive; server capture and encoding choices matter. **Medium** |
| **SPICE drawing commands, caches, image compression, and video streams** | Adapts representation to desktop versus video-like content | Complex server/client stack; primarily useful in virtualized environments. **High** |
| **WebRTC:** H.264, VP8/VP9, AV1; RTP transport | Good hardware-assisted interactive transport | Low-latency settings can reduce compression efficiency. Network loss and congestion adaptation can damage an archive. **Medium–high** |
| **Remote-gaming protocols:** Sunshine/Moonlight, NVIDIA GameStream historically, Steam Remote Play, Parsec | Low host CPU with hardware video; good motion | Often captures a display/session rather than an isolated window. Text fidelity and chroma depend on mode. Parsec’s exact current mode availability **unsure**. **Low** to deploy, **high** to repurpose |
| **Remote framebuffer instructions saved directly** | Compact replay without immediately converting everything to video | Needs a compatible replay engine; a model will usually need reconstructed frames. **Medium–high** |
| **Cloud or second-PC visual analysis** | Can reduce local analysis CPU/RAM | Does not remove capture/transport costs. Storage savings depend on what is retained. **High** |

TCP, UDP, QUIC, RTP, SRT, and RTSP affect delivery and reliability. They are not visual compression schemes.

Similarly, MP4, MKV, WebM, and MPEG-TS are containers. They affect indexing, compatibility, and recovery more than the underlying visual compression.

## 6. Recording policies that materially change the result

| Policy | Benefit across the four goals | Failure points; build cost |
|---|---|---|
| **Variable frame rate/change-driven capture** | Avoids redundant frames and processing | Must preserve timestamps and a maximum observation interval; brief changes can be missed. **Medium** |
| **Adaptive frame rate** | Low overhead while idle, more detail during activity | Motion detection itself costs resources; input alone is an incomplete activity signal. **Medium** |
| **Deduplicate identical frames** | Lossless space saving for static intervals | Comparing/hashing full frames can cost CPU or GPU time. **Low–medium** |
| **Save only input-triggered frames** | Very small files and low overhead | Misses autonomous changes and intermediate feedback. **Low**, but weak as sole evidence |
| **Replay/ring buffer** | Retains only the desired recent clip | RAM ring buffers conflict with low RAM; disk-backed buffers trade RAM for writes. **Low–medium** |
| **Segmented recording with recovery points** | Limits damage from crashes and supports bounded processing | More indexing and segment handling; excessive keyframes increase size. **Medium** |
| **Two-pass or offline recompression** | Can improve final file size after recording | Extra CPU, temporary storage, and delay; poor if total resource use matters. **Low–medium** |
| **Separate live capture from later OCR/model analysis** | Keeps recording CPU/RAM low | Requires retaining enough original evidence and doing work later. **Low–medium** |

A static screen can already compress very well in interframe video. Replacing video with sparse screenshots does not automatically reduce file size enough to justify missed events.

## 7. Useful combinations and real products

These are concrete combinations, not every mathematical permutation. Where I do not know a real product implementing the exact combination, I say so.

| Parts | Combination and product examples | Four-goal result | Failure points; build cost |
|---|---|---|---|
| **2** | **WGC + hardware video encoding.** OBS Studio Window Capture with NVENC, Quick Sync, or AMF where supported | Strong default: high visual quality, small files, low CPU, bounded memory | Codec/hardware settings and window behavior need validation. **Low** |
| **2** | **Rendering hook + hardware video.** OBS Game Capture | Excellent for supported GPU-rendered applications | Injection compatibility; missing external dialogs. **Low** deployment |
| **2** | **Desktop capture + software x264.** OBS Studio | Broad compatibility and controllable quality | CPU use can be high; monitor cropping does not isolate occluded content. **Low** |
| **2** | **Frame capture + animated image export.** ScreenToGif | Convenient for short, simple UI demonstrations | Palette/size limits for GIF; editor memory can grow with clip length. Exact supported export paths vary by version: **unsure**. **Low** |
| **2** | **Video + cursor metadata.** Camtasia | Useful action visibility and editable cursor presentation | Exact metadata and capture internals are proprietary; storage/CPU vary. **Low** using product |
| **2** | **Input steps + screenshots.** Windows Steps Recorder | Compact, readable procedural record | Not continuous evidence. Availability on current Windows builds **unsure**; it is deprecated. **Low** where available |
| **2** | **Workflow actions/selectors + visual references.** UiPath Studio, Power Automate Desktop | Strong action semantics; small records | Authoring/automation recorder, not a faithful passive visual timeline. Exact screenshot retention varies: **unsure**. **Medium** |
| **3** | **WGC + hardware encoder + MKV recording.** OBS Studio | Strong quality/resource balance; MKV improves interruption tolerance | Same capture/codec caveats; later remux may be needed for consumers. **Low** |
| **3** | **Game Capture + hardware encoder + replay buffer.** OBS Studio | Good clip capture without keeping a whole session | Buffer consumes RAM; capture still runs before the clip is saved. **Low** |
| **3** | **Rectangle updates + CopyRect + lossless compression.** TigerVNC/RFB | Excellent static UI and scrolling efficiency | Motion-heavy content becomes expensive; session recording needs extra tooling. **Medium** |
| **3** | **Rectangle updates + lossless compression + JPEG for suitable regions.** TightVNC’s Tight encoding | Balances sharp UI and image-heavy content | JPEG regions are lossy; heuristics and server settings matter. **Medium** |
| **3** | **Remote desktop + instruction recording + replay.** Apache Guacamole | A compact, replayable remote-session record | Captures the remote session, with fidelity depending on its protocol path; not an arbitrary local HWND recorder. **Medium** |
| **3** | **Actions + DOM snapshots + screenshots.** Playwright Trace Viewer | Excellent for understanding web workflows; exact text plus visual context | Browser automation only; screenshots are not full continuous video. **Low–medium** |
| **4** | **Actions + DOM snapshots + screenshots + network evidence.** Playwright tracing | Strong browser debugging record with compact structural evidence | Does not cover arbitrary Windows applications or every transient visual state. **Low–medium** |
| **4** | **Drawing commands + image resources/cache + video streams + input.** SPICE, used with QEMU/KVM and virt-viewer | Efficient mixed desktop representation | Complex stack and guest support; recording/replay is additional work. **High** |
| **4** | **Capture + hardware encode + network transport + client hardware decode.** Sunshine/Moonlight | Low host CPU and good motion at manageable bandwidth | Whole-display capture concerns; recording and model playback need added integration. **Low** deploy, **high** custom |
| **4** | **WGC + hardware video + UIA events/text + lossless detail snapshots.** Custom recorder using WGC, vendor encoder/MF, UIA, PNG | Best general architecture for AI evidence in my judgment: continuous visuals, exact exposed text, recoverable detail | Synchronization, popup handling, tree filtering, screenshot policy. I do not know a mainstream product that guarantees this exact complete package. **High** |
| **4** | **Changed tiles + move operations + lossless compression + periodic full frames.** A custom RFB-like recorder | Excellent potential for exact, mostly static UI with small storage and bounded memory | Reconstruction complexity and poor high-motion behavior. VNC provides related mechanisms; exact complete archive product **unsure**. **High** |

Two additional combinations matter:

- **Periodic screenshots + OCR + searchable timeline:** Windows Recall is a real example of this general architecture. It is not a continuous clip recorder. Current implementation details, capture frequency, and export suitability **unsure**.
- **Browser resources + interaction/DOM recording + replay:** products such as FullStory and LogRocket use this general approach for web session replay. Exact capture coverage and compression are proprietary. They do not record arbitrary native Windows windows.

## 8. Top five candidates across all four goals

This is a conditional ranking for **an arbitrary application window**, with later AI inspection as the purpose. It is not a benchmark result.

### 1. WGC + hardware AV1 or HEVC + selective UIA logging + lossless detail snapshots

**Best overall architecture.**

Continuous video preserves visible activity. UIA supplies exact text and state when available. Lossless snapshots preserve important detail that the video or model’s resizing might otherwise lose.

- **Quality:** strongest combined visual and semantic evidence.
- **Files:** small video plus usually small sidecars; snapshots require restraint.
- **CPU:** low encoding CPU; bounded event handling; defer OCR.
- **RAM:** bounded capture/encoder queues and no accumulating screenshot list.
- **Failure:** highest implementation complexity; UIA can be costly or incomplete.
- **Build:** high.

Choose AV1 versus HEVC by testing the actual hardware encoders. I would not choose solely from codec reputation.

### 2. WGC + hardware AV1 or HEVC alone

**Best simple visual recorder.** Start with OBS to establish the achievable baseline.

- **Quality:** very good with native resolution and suitable settings.
- **Files:** generally compact.
- **CPU:** low with an actual hardware path.
- **RAM:** modest and bounded when configured sensibly.
- **Failure:** exact small text can still suffer, especially after 4:2:0 conversion and model-side resizing.
- **Build:** low through OBS; medium custom.

For applications with little accessible structure, this may be practically as useful as candidate 1.

### 3. WGC + hardware H.264 + selective text/screenshot sidecars

**Best compatibility-oriented choice.**

- **Quality:** strong with enough bitrate and detail snapshots.
- **Files:** often larger than comparable HEVC/AV1, but workload dependent.
- **CPU/RAM:** usually low with mature hardware support.
- **Failure:** high-quality desktop text can require more storage; sidecars add complexity.
- **Build:** medium–high custom; low for the video baseline.

This can outrank AV1/HEVC on an older PC or an ingestion system with limited decoder support.

### 4. Lossless changed tiles + move records + periodic full frames

**Best specialist choice for mostly static desktop UI and exact pixels.**

- **Quality:** exact reconstruction.
- **Files:** potentially excellent on forms, editors, dashboards, and repeated views.
- **CPU:** potentially low with source dirty/move metadata or GPU-assisted comparisons.
- **RAM:** bounded previous-frame/tile state.
- **Failure:** scrolling, animated canvases, embedded video, and noisy content can erase the advantage.
- **Build:** high.

A simple PNG sequence is easier, but it leaves considerable compression efficiency unused.

### 5. Change-driven screenshots + UIA/event timeline

**Best when resource use matters most and missed intermediate states are acceptable.**

- **Quality:** excellent sampled frames and exposed text; incomplete temporal evidence.
- **Files:** often very small.
- **CPU/RAM:** usually low with careful event-driven capture.
- **Failure:** brief errors, loading indicators, animations, and rapid changes may disappear entirely.
- **Build:** medium–high.

If “read back what happened” means preserving every visible transition, this candidate is disqualified. Move continuous video ahead of it regardless of its resource savings.

**For strictly exact visual evidence**, candidate 4 moves upward. **For fast-changing content**, hardware video wins more convincingly. **Without a suitable hardware encoder**, the ranking needs to be redone.

## 9. Settings and implementation choices that can outweigh codec choice

- **Keep native text resolution.** Downscaling can destroy more useful information than moderate codec compression.
- **Test chroma handling.** Common 4:2:0 paths reduce color resolution. Colored one-pixel text and syntax highlighting are good stress cases. Hardware 4:4:4 support is device/profile dependent.
- **Use quality-targeted encoding where available.** Fixed bitrate wastes space on idle screens and can starve difficult moments. Vendor quality controls are not directly interchangeable.
- **Keep queues bounded.** More queued frames mean more RAM and delay, not automatically better quality.
- **Stream to disk.** Do not accumulate raw frames or screenshots for the whole clip.
- **Preserve timestamps.** Video, UIA events, input, and screenshots need one coherent timeline.
- **Preserve cursor evidence.** Record or reconstruct it correctly; include click events if useful.
- **Include related windows deliberately.** Menus, tooltips, dialogs, and file pickers may be separate windows.
- **Retain an original visual record.** OCR and model summaries are useful indexes, but mistakes become permanent if originals are discarded.
- **Plan model ingestion early.** The model may receive sampled/resized images rather than your original video. A perfect archive can still produce poor AI understanding.

## 10. What I am unsure about

- Which hardware encoder wins on **this PC**, for **this application**, at equivalent text readability.
- Whether AV1’s storage benefit over HEVC is meaningful for the actual desktop workload.
- Whether the available GPU supports useful high-fidelity chroma modes through the chosen API.
- Current product-specific capture paths, export formats, and licensing details not explicitly established above.
- How reliably the target window behaves under occlusion, minimization, resizing, popups, HDR, and DPI changes.
- Whether UIA exposes useful information without expensive traversal or excessive events.
- How much the model’s frame sampling and resizing will dominate the result.
- Whether exact pixels, exact text, or complete temporal coverage matters most to the intended later questions.

## 11. What must be measured on a real PC

Use the same short workflow for each candidate: idle text, typing, scrolling, menus, dialogs, resizing, rapid changes, and embedded video if relevant.

Measure:

| Measurement | Why it decides the choice |
|---|---|
| **Actual model answers** to known questions about the clip | The real objective is correct understanding, not attractive playback |
| **Small-text transcription accuracy**, including colored text | Reveals chroma, scaling, and compression damage |
| **Brief-event detection and action order** | Reveals sampling gaps and timestamp errors |
| **File size for identical recorded time/content** | Establishes actual compression efficiency |
| **Average and peak CPU** | Peaks can interfere with the application even when averages look low |
| **System RAM, private working set, VRAM/shared GPU memory** | Prevents “low RAM” claims that merely move buffers elsewhere |
| **GPU encode/copy/3D load and application responsiveness** | Hardware encoding still competes for resources |
| **Disk throughput and writes** | Lossless or raw recording may shift the bottleneck to storage |
| **Dropped, duplicated, stale, or black frames** | Establishes recording integrity |
| **Occlusion, minimize/restore, DPI, HDR, popup behavior** | Establishes whether one-window capture actually works |
| **Decode/seek/model-ingestion cost** | Small archives can still be expensive to inspect |
| **Crash/interruption recovery and long-run memory growth** | Establishes whether a short successful demo remains reliable |

Start with **OBS WGC plus each available hardware encoder**. That gives a cheap, concrete baseline. Build the semantic sidecars or lossless tile recorder only when those measurements show a benefit.