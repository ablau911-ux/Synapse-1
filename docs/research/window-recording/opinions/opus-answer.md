> **OPINION, NOT PROOF.** Answer of Claude Opus 5.5 to the bare task (file `question.md`), given in an empty folder on 2026-11-23, from the model's own knowledge. Nothing here is a finding until it is proven from a source in `../proof.md`.

# Recording one app window on Windows so an AI can read back what happened

This is from my own knowledge only. I measured nothing and opened no files. Any number marked **unsure** is a rough estimate.

## Core finding

AI models don't read video the way people watch it.
- **Claude and GPT:** they take still images plus text. To use a video you must cut it into frames.
- **Gemini:** it accepts video files directly, but it samples them at about 1 frame per second. **Unsure** on the exact rate and token cost per frame.

So recording 30 or 60 frames per second mostly stores frames the model never sees. The best results come from three parts together:
- a **text log of what happened**, taken from the accessibility tree and input events;
- **still images taken only when the window changes**;
- **video only for parts that really move.**

Video codecs matter less than the decision about *what* to record.

---

## 1. Every fundamentally different method

Ratings: **Q** = how well the AI can read it, **Size** = file size, **CPU**, **RAM**. Scale: ++ (very good), + , 0 , − , −− (very bad).
Build cost: **S** = hours to days, **M** = 1–4 weeks, **L** = months.

### A. Ways to capture pixels

| # | Method | Q | Size | CPU | RAM | Where it fails | Build |
|---|---|---|---|---|---|---|---|
| A1 | **Windows.Graphics.Capture (WGC)**, using `GraphicsCaptureItem` for one window (from `CreateForWindow` on an HWND) | ++ | n/a | ++ (the frame stays on the GPU as a D3D11 texture) | ++ (2–3 textures in the frame pool, about 8 MB each at 1080p) | **Misses popups, menus, tooltips and dropdowns**, because those are separate top-level windows (fairly sure). No frames while minimized. Windows protected with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` and DRM video come out black. Shows a yellow border, which `IsBorderRequired=false` removes on Windows 11. Frame size changes when the window is resized. HDR needs FP16 frames and tone-mapping. `DirtyRegionMode` and `MinUpdateInterval` only exist on newer Windows 11 builds (**unsure** which build: maybe 24H2). | S–M. Libraries: robmikh `Win32CaptureSample`, the Rust/Python `windows-capture` crate, .NET `ScreenRecorderLib`. |
| A2 | **DXGI Desktop Duplication** (`IDXGIOutputDuplication`): capture the whole monitor, then crop to the window | ++ | n/a | ++ | ++ | Captures whatever is on screen, so **anything covering the window gets recorded too**. Lost when UAC or the secure desktop appears and during fullscreen mode switches. On hybrid-GPU laptops it must run on the GPU that drives that monitor. Advantage: it **gives dirty rectangles and move rectangles for free** (scroll detection) and does catch popups. | S–M. FFmpeg `ddagrab` filter (FFmpeg 6+). |
| A3 | **GDI**: `BitBlt` or `PrintWindow(PW_RENDERFULLCONTENT)` | + | n/a | −− (copies run on the CPU, about 10–30 ms per frame at 1080p, **unsure**) | + | GPU, DirectComposition and Chromium apps often come out black unless that flag is set. It can force the app to repaint. Slow. Its advantage is that it works on covered windows. | S. FFmpeg `gdigrab`. |
| A4 | **Graphics API hooks**: inject into D3D/GL/Vulkan `Present` (OBS "Game Capture") | ++ | n/a | ++ | + | Only works for apps that draw with D3D, GL or Vulkan. It means DLL injection, so anti-cheat and security tools may block it and the target app can crash. | L (or reuse OBS). |
| A5 | **Magnification API** (`MagSetImageScalingCallback`) | + | n/a | 0 | + | Deprecated and has quirks. Its only real use is leaving certain windows out of a capture. | M |
| A6 | **Virtual display**: an Indirect Display Driver (IDD), the successor to mirror drivers, which ended with Windows 8 | ++ | n/a | + | + | Needs a signed driver, and the app has to be placed on the virtual monitor. Overkill here. Used by Parsec VDD and some remote tools. | L |
| A7 | **Built-in Windows recorders**: Xbox Game Bar (GameDVR), Snipping Tool video, `Windows.Media.AppRecording` | + | 0 | ++ (hardware encoder) | + | Little control: H.264 4:2:0, fixed frame rate, no event log. Game Bar refuses File Explorer and the desktop. `AppRecording` is meant for an app recording itself (**unsure**). | S (as a user) |

### B. Video codecs and encoders

Screen content is mostly flat colours, sharp text and large areas that don't change. Codecs tuned for camera footage waste bits on it.

| # | Codec or encoder | Q for text | Size | CPU | RAM | Where it fails | Build |
|---|---|---|---|---|---|---|---|
| B1 | **H.264 hardware**: NVENC, Intel Quick Sync (QSV/oneVPL), AMD AMF, Media Foundation `*_mf` MFTs | 0 to + | + | ++ (1–3% CPU, **unsure**) | + (about 50–200 MB, **unsure**) | 4:2:0 chroma blurs small coloured text and ClearType edges. Rate control isn't tuned for screen content. NVENC supports H.264 4:4:4, but AMF and QSV mostly don't (**unsure**). Chromium can't decode H.264 4:4:4. | S (FFmpeg) or M (MF Sink Writer) |
| B2 | **HEVC hardware**, with Main 4:4:4 on NVENC Turing and later and on Intel Gen11 and later (**unsure** on exact generations) | + | ++ | ++ | + | The 4:4:4 profile isn't supported everywhere, and HEVC playback on Windows needs the paid "HEVC Video Extensions". Hardware encoders don't implement the screen-content tools (SCC: palette mode and intra block copy) (**unsure** on Intel). | S–M |
| B3 | **AV1 hardware**: NVENC on Ada and Blackwell, Intel Arc and Meteor Lake and later, AMD RDNA3 and later | + | ++ | ++ | + | Only on new GPUs. Mostly 4:2:0, and most of these encoders leave the AV1 screen-content tools out (**unsure**). | S |
| B4 | **x264 in software**: `-tune stillimage` or `animation`, `-pix_fmt yuv444p`, or `libx264rgb` | ++ (4:4:4) | ++ | − (about 0.5–2 cores at 1080p30 with veryfast, **unsure**) | 0 (lookahead uses 100–500 MB) | CPU cost grows with resolution and frame rate. At low fps with VFR it is cheap. | S |
| B5 | **x265 in software** | ++ | ++ | −− | − | Slow. x265 has no SCC tools (fairly sure). | S |
| B6 | **AV1 in software with screen-content tools**: libaom `--tune-content=screen`, SVT-AV1 `--scm 1`, rav1e. These have palette mode and IntraBC. | ++ | ++ (best for text per byte among common codecs) | −− (libaom) to − (SVT-AV1 fast presets) | − | Too heavy for live 30 fps on weak CPUs. Fine for low fps, or for transcoding later. | S |
| B7 | **VP9** (libvpx `--tune-content=screen`), VP8 | + | + | − | 0 | Weaker than AV1. VP9 screen-share mode is what WebRTC uses. | S |
| B8 | **VVC/H.266** with IBC and palette tools (VVenC) | ++ | ++ | −− | −− | Few decoders. FFmpeg gained a VVC decoder in 7.x, but AI tools mostly won't ingest VVC. | M |
| B9 | **HEVC-SCC reference encoder** (HM-SCM) | ++ | ++ | −−− | −− | Research-grade speed. Not practical. | L |
| B10 | **Lossless intermediate codecs**: Ut Video, MagicYUV, FFV1, HuffYUV, Lagarith, x264 `-qp 0` with ultrafast | +++ | −− (roughly 50–300 Mbps at 1080p, **unsure**; screen content does better) | + (Ut Video and MagicYUV are very fast) | + | Big files. Only useful as stage one before a later transcode. | S |
| B11 | **Old screen codecs**: TechSmith TSC2 (Camtasia), ZMBV (DOSBox), CamStudio Lossless, MSU Screen Codec, Microsoft MSS1/MSS2, Flash FSV1/FSV2, GoToMeeting G2M, VMware VMnc | ++ (lossless) | + for static screens | + | + | Proprietary or dead, poor decoder support, AI tools can't read them. They show that coding only changed regions works. | S–M |
| B12 | **MJPEG, GIF, APNG** | 0 / − (GIF has a 256-colour palette) | −− | + | + | Big files or banding. Only for compatibility. | S |
| B13 | **Neural codecs** (Microsoft DCVC family, NVIDIA Maxine for faces only) | ? | ++ in papers | −− (needs GPU inference) | −− | Research only. No stable format. | L |

### C. Still images instead of video

| # | Method | Q | Size (1080p UI frame, **unsure**) | CPU per frame | Where it fails | Build |
|---|---|---|---|---|---|---|
| C1 | **PNG** (zlib, or fast encoders `fpng` and `fpnge`) | +++ (lossless) | 100–500 KB | zlib: 20–60 ms; fpnge: a few ms | Bigger than the newer formats below | S |
| C2 | **WebP lossless** | +++ | about 25–40% smaller than PNG | 50–300 ms depending on method (**unsure**) | Slow encoder at high effort | S |
| C3 | **JPEG XL lossless** (libjxl; effort 1 is the very fast "fjxl" path) | +++ | about 35–50% smaller than PNG | effort 1–2: very fast | Claude, GPT and Gemini don't accept JXL as input (fairly sure), so frames must be converted to PNG or WebP before sending | S |
| C4 | **Lossy WebP or JPEG at high quality**, and AVIF | ++ | 40–150 KB | fast (AVIF slower) | JPEG rings around text, and 4:2:0 hurts coloured text | S |
| C5 | **QOI** | +++ | about PNG size | extremely fast | No support in AI APIs | S |
| C6 | **Changed tiles only** (store dirty rectangles as small PNG/WebP tiles with coordinates, like VNC/RFB) | +++ | ++ | + | Needs a step that rebuilds full frames before the AI sees them | M |

### D. Structural and event recording (not pixels)

| # | Method | Q for "what happened" | Size | CPU / RAM | Where it fails | Build |
|---|---|---|---|---|---|---|
| D1 | **UI Automation (UIA)**: tree snapshots plus events (`FocusChanged`, `StructureChanged`, `PropertyChanged`, `TextChanged`, `Invoke`, `SelectionItem`). Libraries: FlaUI (.NET), `uiautomation`/pywinauto (Python), the `UIAutomationCore` COM API (C++) | +++ for buttons, labels, values, focus and text. Zero for visuals. | ++ (KB per minute) | Calls cross processes and are slow (around 1 ms or more each, **unsure**). Big trees cost CPU in both processes, so use `CacheRequest` and limited tree walks. | Blind to custom-drawn UI, games, canvas, charts and images. Qt without accessibility, Java Swing without the Access Bridge and some Electron apps expose little. Connecting a UIA client turns on Chromium's accessibility mode, which slows the app. | M |
| D2 | **WinEvents** (`SetWinEventHook` with `EVENT_OBJECT_*` and `EVENT_SYSTEM_FOREGROUND`) and MSAA/`IAccessible` | ++ | ++ | ++ (cheap when out-of-context) | Coarser than UIA | S |
| D3 | **Input log**: `WH_KEYBOARD_LL`, `WH_MOUSE_LL` or Raw Input, with timestamps and click coordinates mapped to the UIA element under the cursor | +++ for causality ("clicked Save") | ++ | ++ | Privacy: this is a keylogger. Passwords must be masked (UIA `IsPassword`). Security software may flag it. | S |
| D4 | **Window metadata**: title, process, rect, DPI and focus changes, from Win32 calls or ETW Win32k events (**unsure** on exact providers) | + | ++ | ++ | Thin on its own | S |
| D5 | **OCR on frames**: `Windows.Media.Ocr` (built in), the Windows App SDK text recognition API on Copilot+ PCs (NPU), Tesseract, PaddleOCR | ++ for text | ++ (text only) | Windows OCR is moderate; Tesseract is heavy | Mistakes on small or stylised text, loses layout. It duplicates the pixels the model reads anyway. | S–M |
| D6 | **App-specific object models**. Browser: CDP DOM, rrweb DOM-mutation recording, Playwright traces. Terminal: ConPTY output to an asciicast v2 file (asciinema). Office: COM (`Word.Application` events). | +++ perfect fidelity | +++ | ++ | Works only for that one kind of app | M each |
| D7 | **Drawing-command recording**: GDI to EMF (Enhanced Metafile), or GPU frame capture with PIX or RenderDoc | + | GDI: + ; GPU capture: −−− | GPU capture: −− | GDI-only apps barely exist now. GPU captures are huge debugging dumps. | L |
| D8 | **System tracing**: ETW (WPR), Process Monitor, Spy++ message logs | 0 for UI meaning | − | 0 | Shows that a file was saved, but not what the user saw | S |
| D9 | **App audio**: WASAPI per-process loopback (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, Windows 10 2004 and later), with Whisper or Windows speech-to-text | + when the app has sound | ++ (Opus at 16–32 kbps) | Whisper is heavy, Opus is cheap | Optional | S–M |
| D10 | **Captions or embeddings on the fly** (a local vision model such as Florence-2, Phi-4-multimodal or Qwen2.5-VL writes captions; or store CLIP/SigLIP embeddings) | captions: + ; embeddings: −− (can't be turned back into text) | +++ | −−− (GPU, gigabytes of RAM) | Throws away detail before the real model sees it, and the local model makes mistakes | M |

### E. Streaming and remote-display techniques

These all show the same trick: send changed regions, use a lossless or near-lossless codec for text, and use video only for moving areas.

| # | Technique | Key idea | Notes |
|---|---|---|---|
| E1 | **RDP graphics pipeline** (MS-RDPEGFX): AVC420/AVC444, RemoteFX Progressive, ClearCodec, Planar | Per-region choice of codec. AVC444 is built from two 4:2:0 streams. Progressive starts blurry and sharpens to lossless. | You can record an RDP session at the protocol level. Products: Apache Guacamole session recording (convert to video with `guacenc`), Teleport desktop access, CyberArk PSM. To record your own app this way you must run it inside an RDP session: build L. |
| E2 | **VNC/RFB**: Tight, ZRLE and Hextile encodings | Changed rectangles, palette and zlib | Tiny for static UI. Products: TightVNC, TigerVNC, `vnc2flv`/`vnclog`-style recorders (**unsure** on current tools). |
| E3 | **Citrix HDX Thinwire** ("build to lossless"), **VMware Blast Extreme**, **Teradici/HP Anyware PCoIP** | Text regions get lossless or JPEG/PNG; video regions get H.264 | Shows the hybrid is proven at scale |
| E4 | **SPICE/QXL** | Sends drawing commands | Mainly for virtual machines |
| E5 | **Game streaming**: Parsec, Sunshine plus Moonlight, NVIDIA GameStream | Desktop Duplication or WGC feeding NVENC at low latency | Built for latency, not size |
| E6 | **WebRTC screen share** (`getDisplayMedia`; VP9 or AV1 in screen mode, with "detail" content hint) | Screen-content tools, adaptive fps | Chrome Remote Desktop, Teams and Meet |
| E7 | **RustDesk, NoMachine NX** | VP8, VP9, AV1 or H.264/H.265; NX also caches drawing commands | Open source (RustDesk) |

---

## 2. Combined solutions and the products that use them

### Pairs
| Combination | Products (real, as far as I know) |
|---|---|
| WGC + hardware H.264/HEVC/AV1 | OBS (Window Capture with the "Windows 10 (1903 and up)" method, plus NVENC/QSV/AMF), Snipping Tool video recording (**unsure** whether it uses WGC), `ScreenRecorderLib` |
| Desktop Duplication + NVENC | NVIDIA ShadowPlay/Instant Replay, Parsec, Sunshine, FFmpeg `ddagrab` → `h264_nvenc` |
| gdigrab + x264 | Many FFmpeg scripts. ShareX's FFmpeg recorder (**unsure** of the current default source) |
| Screenshot on each click + UIA element description | **Windows Steps Recorder (`psr.exe`)**, now deprecated, which saved MHT files. Scribe and Tango (mostly in the browser). UiPath Task Capture. |
| Periodic or on-change screenshot + OCR | **Windows Recall**, **Screenpipe** (open source), Rewind/Limitless (Mac) |
| DOM mutations + input events | rrweb, Sentry Session Replay, LogRocket, FullStory, PostHog |
| Terminal output stream + timing | asciinema |
| Fast lossless capture + later transcode | OBS "lossless" recording followed by HandBrake or FFmpeg (a common workflow) |

### Triples
| Combination | Products |
|---|---|
| Change-triggered screenshots + keystroke/mouse log + window titles | Proofpoint ITM (formerly ObserveIT), Syteca (formerly Ekran System), Teramind, ActivTrak |
| Screenshots + OCR + semantic index (embeddings on the NPU) | Windows Recall, Screenpipe |
| DOM snapshots + screenshots + action log | Playwright Trace Viewer |
| Screenshots + UIA tree + input actions | OpenAdapt; Microsoft UFO/UFO² agent (for control rather than recording, but the same data shape); Power Automate Desktop recorder; UiPath Task Mining; Celonis Task Mining (**unsure** on exact internals) |
| Video + audio + speech-to-text transcript | Teams, Zoom and Loom recordings |

### Four parts
| Combination | Products |
|---|---|
| WGC frames sent only on change, as lossless stills and/or low-fps screen-tuned video, + UIA events/tree + input log + window metadata | Closest: OpenAdapt and the task-mining tools. No mainstream product does all of it well for one window (**unsure**). **This is my recommended design.** |
| Video + OCR + speech-to-text + search index | Screenpipe, Rewind |
| RDP protocol recording + keystrokes + OCR + video export | Privileged-access tools: CyberArk PSM, BeyondTrust, Teleport (**unsure** on OCR in each) |

---

## 3. Notes on the four goals across all methods

- **Quality for AI reading** depends on, in order:
  1. having text as text (UIA, DOM, OCR);
  2. sharp frames at the size the model actually uses. Claude scales images down to about 1568 px on the long side (about 1.15 MP), and GPT and Gemini resize or tile too;
  3. 4:4:4 colour, or lossless frames;
  4. catching the frame where something changed.

  A higher frame rate barely helps.
- **Small files:** the biggest lever is **not recording when nothing changes**: variable frame rate, or frames only on change. That beats switching codecs by 10–100×. Next come downscaling to the model's input size and using a screen-tuned codec.
- **Low CPU:** keep frames on the GPU. That means WGC or Desktop Duplication, a GPU-side frame-difference check (a compute shader, or the dirty rectangles Windows provides), and a hardware encoder. Avoid GDI, software encoding at 30 fps, and deep UIA tree walks.
- **Low RAM:** small frame pool, no long encoder lookahead, write to disk as you go, and use cached UIA queries.

---

## 4. Top five candidates (all four goals together)

**1. Hybrid event log plus stills on change.** I recommend this one.
- **Parts:** WGC capture of the window, plus a crop of the monitor for popups. A GPU check (or dirty rectangles) decides whether the frame changed. Once the screen settles for about 150–300 ms, save one frame scaled to about 1.1–1.5 MP as lossless WebP or PNG (fpnge for speed). Alongside it, write a JSONL timeline of UIA/WinEvent events, input actions mapped to UIA elements, and window titles and positions.
- **Why it ranks first:**
  - It gives the AI the precise text ("clicked *Save* in dialog *Export*; field *Name* = 'report.pdf'") plus the exact frames that matter.
  - It typically comes to well under 1 MB per minute of normal UI work (**unsure**, needs measuring).
  - CPU is near zero when idle, and RAM is tens of MB.
- **Where it fails:** smooth motion such as video, animations and drag paths, and apps with poor UIA support. Those fall back to pixels only.
- **Build:** M, about 3–6 weeks for something robust.

**2. WGC + hardware encoder at low variable frame rate, plus a side-car event log.** HEVC or AV1, 4:4:4 where the GPU supports it, constant-QP tuned for text, about 2–5 fps maximum, frames dropped when nothing changed (FFmpeg `mpdecimate`, or a check at capture time).
- **Why:** it handles motion, costs about 1–3% CPU, and the files stay small.
- **Where it fails:** 4:2:0 blurs small coloured text on many GPUs. Claude and GPT need frames extracted. HEVC licensing on Windows.
- **Build:** S–M.

**3. Steps-Recorder-style stills only, triggered by input events.** One frame on each click, key burst or focus change, plus the UIA name of the target.
- **Why:** the simplest and smallest option, with near-zero CPU, and exactly the "what happened" shape.
- **Where it fails:** changes that happen without input (progress bars, background updates). Easy to add a slow dirty-region trigger to fix that, which turns it into #1.
- **Build:** S–M.

**4. Two-stage recording with screen-tuned software encoding.**
- **Stage 1:** record cheaply (hardware encoder at near-lossless constant QP, or Ut Video).
- **Stage 2:** when the PC is idle, transcode with SVT-AV1 `--scm 1` or x264 4:4:4 `-tune stillimage`.
- **Why:** best video quality per byte at low live CPU.
- **Where it fails:** temporary disk use, a delay before files are final, and the AV1 transcode is CPU-heavy (it just runs later).
- **Build:** S.

**5. App-specific structural recording where it applies:** CDP or rrweb for browsers and Electron, asciicast for terminals, COM for Office.
- **Why:** perfect text fidelity, files measured in KB, almost no CPU.
- **Where it fails:** each recorder works for only one kind of app.
- **Build:** M per app type.
- **Best use:** plug it into #1 as an extra source when the target app is one of these.

Not in the top five:
- Plain 30 fps H.264 4:2:0 recording (OBS default style): it wastes size and blurs text.
- GDI capture: too much CPU and black frames.
- Hooks and drivers: risky and fragile.
- Local captioning models: heavy, and they lose detail.
- RDP or VNC protocol recording: excellent compression, but you would have to run the app inside a remote session.

---

## 5. What I'm unsure of, and what to measure on the real PC before choosing

**Unsure (from memory, not verified):**
- Whether WGC window capture includes owned popups and menus. I believe it doesn't.
- Which Windows build added `DirtyRegionMode` and `MinUpdateInterval` to WGC.
- Hardware 4:4:4 and screen-content-tool support per GPU generation (NVENC, QSV, AMF).
- All the size and CPU numbers above.
- Gemini's video sampling rate and token cost per frame.
- Which AI APIs accept which image formats (I believe PNG, JPEG, WebP and GIF; not JXL or AVIF).
- The internals of the task-mining products and Snipping Tool.

**Measure before choosing:**
1. **GPU and encoder:** which of NVENC, QSV and AMF exist, and whether they support HEVC/AV1 and 4:4:4. Check with `ffmpeg -encoders` and run a trial encode.
2. **Readability:** for each pipeline, decode the frames and compare against lossless frames:
   - character error rate with `Windows.Media.Ocr`;
   - the target model's accuracy on a fixed set of questions about the clip.

   Test at 100% and 150% display scaling, and with ClearType on and off.
3. **Bytes per minute in four scenarios:** idle, typing, scrolling a long list, and video playing in the window.
4. **CPU %, GPU encode use and RAM** for the recorder process *and* for `dwm.exe` *and* for the target app, which slows down with UIA and Chromium accessibility. Measure on AC power and on battery.
5. **UIA coverage** of the actual target apps: what share of visible text UIA exposes, the event rate, and the cost per query.
6. **WGC behaviour:** popups, resizing, DPI changes, minimizing, multi-monitor, HDR, protected windows, the border, and the cursor.
7. **Change detection:** what settle delay catches the final state without missing short states. Also how many frames per minute come out in practice.
8. **Model token cost per minute** at the chosen frame size and count, compared with sending text only.
9. **Privacy:** whether password fields and secret windows are reliably masked before anything is written to disk.
