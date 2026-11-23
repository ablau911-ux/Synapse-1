# Full list of subjects to review and research

**Task:** record what happens in an application window on Windows so that an AI model can read it back. Goals: maximum quality, small files, low CPU, low RAM.

No method is assumed. Video coding is one family among many. Status: **open** = not searched yet; **partly** = some findings logged; the numbers point to findings in `research-log.md`.

## 1. The reader: what "quality" means for an AI
| Subject | Status |
|---|---|
| How models take in images: size limits, downscaling, token cost | partly (9) |
| How models take in video: frame sampling, native video input | open |
| How small and how compressed text can be before a model misreads it | partly (10) |
| What a model needs to understand an event: before/after pictures, differences, timing, text | open |
| When exact pixels matter (rendering faults, colours) and when readable is enough | open |
| Storing what the model consumes instead of pixels: text, layout, image embeddings or tokens | open |

## 2. What can be recorded at all (sources of truth about a window)
| Subject | Status |
|---|---|
| Pixels of the window | partly (7) |
| The UI tree and its events (UI Automation, accessibility) | open |
| Web page structure and events (DOM, DevTools protocol) | partly (11, 19, 21) |
| Input: keyboard, mouse, touch, focus, window moves | open |
| Drawing commands: GDI/metafile, DirectX call capture (apitrace, RenderDoc), RDP drawing orders, terminal recorders | open |
| System traces (ETW), application logs, network activity | open |
| Text by OCR | partly (1) |
| Deterministic replay: record inputs and replay the program (time-travel debugging) | open |
| Audio | open |

## 3. Ways to capture pixels on Windows
| Subject | Status |
|---|---|
| Windows.Graphics.Capture | partly (7, 22) |
| Desktop Duplication | partly (23) |
| GDI BitBlt and PrintWindow | open |
| Desktop Window Manager routes: thumbnails, shared surfaces | open |
| Magnification API | open |
| Hooks inside the application's graphics (game capture) | open |
| Mirror drivers and indirect display drivers | open |
| Capture cost: CPU, GPU, RAM, power; covered, minimized and protected windows; scaling and HDR | open |

## 4. Knowing what changed
| Subject | Status |
|---|---|
| Changed and moved rectangles from the system | partly (7, 23) |
| Comparing frames: whole-frame hash, tile hash, pixel difference, perceptual difference | open |
| Scroll and move detection | open |
| Separating the cursor | open |
| Event-driven capture (capture when something happens, not on a clock) | partly (1, 2) |

## 5. Ways to represent a recording
| Subject | Status |
|---|---|
| Continuous video | partly |
| Still pictures only when something changes | partly (2, 21) |
| Changed rectangles or tiles, each coded separately (remote desktop style) | partly (12, 17, 18) |
| Layers by content type: exact text, lossy pictures (mixed raster content) | partly (12, 20) |
| Structure and events instead of pixels | partly (11, 19) |
| Drawing-command recording | open |
| Text and layout reconstruction (OCR or UI tree as the main record) | open |
| Record fast now, compress better later in the background | open |
| Combinations of the above | open |

## 5b. Combined solutions: two, three and four methods together (owner request)

Building blocks that can be combined:
- **V** video stream of the window
- **S** exact still pictures when the screen settles
- **R** exact changed rectangles
- **T** text and structure track (UI tree, web page structure)
- **E** event timeline (keys, mouse, focus, window changes)
- **O** OCR text index
- **L** application logs (console, network)
- **A** audio

| Subject | Known example | Status |
|---|---|---|
| **Two:** V + S (video while changing, exact still when settled) | Citrix "build to lossless" (36) | partly |
| **Two:** R + V (changed rectangles, video only for moving areas) | RDP (12), Citrix selective mode (36) | partly |
| **Two:** S + T (stills on change plus text) | Microsoft Recall (2) | partly |
| **Two:** S + E (one still per user action plus a note) | Windows Steps Recorder (21) | partly |
| **Two:** V + E, V + T | | open |
| **Three:** V + S + T | | open |
| **Three:** V + T + O | screenpipe (1) | partly |
| **Three:** R + V + text codec | RDP (12) | partly |
| **Three:** S + T + E | agent run records (34) | partly |
| **Four:** V + S + T + E | | open |
| **Four:** T + S + V + L | Playwright trace: page structure, stills, video, console and network (11) | partly |
| **Four:** V + T + O + A | screenpipe with audio (1) | partly |
| How the parts are kept in step (one clock, one index) and read back together | | open |
| Cost of each added part in size, CPU and RAM; which part the reader uses first | | open |

## 6. Video coding standards (all of them)
| Subject | Status |
|---|---|
| H.264/AVC | partly (5, 16) |
| H.265/HEVC, with its range and screen content extensions | partly (3) |
| H.266/VVC and its screen tools | partly (3) |
| AV1 and its screen tools; AV2 | partly (6, 8) |
| VP8, VP9 | open |
| EVC, LCEVC, AVS2/AVS3, VC-1, Theora, Dirac/VC-2, MPEG-2, MPEG-4 Part 2 | open |
| Features that matter for screens: palette, intra block copy, transform skip, adaptive colour transform, long-term references, skip of unchanged blocks | partly (3, 6) |

## 7. Codecs made for screens
| Subject | Status |
|---|---|
| TechSmith TSCC and TSC2 (Camtasia) | partly (17) |
| ZMBV (DOSBox) | partly (17) |
| Windows Media Video 9 Screen (MSS1, MSS2) | partly (17) |
| Flash Screen Video 1 and 2, CamStudio, QuickTime Animation, Fraps, GoToMeeting G2M, VMware VMnc | open |
| Screen codecs inside conferencing products (Zoom, Webex, Teams) | open |

## 8. Lossless and editing-grade video codecs
| Subject | Status |
|---|---|
| Lossless H.264/HEVC/AV1 modes | partly (15) |
| FFV1, UT Video, MagicYUV, Lagarith, HuffYUV | open |
| ProRes, DNxHR, CineForm, Motion JPEG, JPEG XS | open |

## 9. Still-image and animated-image formats
| Subject | Status |
|---|---|
| PNG, WebP, JPEG XL, AVIF, HEIC, QOI, JPEG, JPEG 2000 | partly (13) |
| Indexed-colour and palette images for UI | open |
| GIF, APNG, animated WebP, animated AVIF | open |

## 10. General compressors and delta methods
| Subject | Status |
|---|---|
| zstd, LZ4, Brotli, xz, Deflate; long-window and dictionary modes | open |
| Delta coding between frames; block de-duplication; content-defined chunking | open |

## 11. Learned and semantic compression
| Subject | Status |
|---|---|
| Neural image and video codecs; their speed on a PC | open |
| Text-aware and OCR-aware compression | partly (10) |
| Super-resolution on read-back (store small, restore sharp) | open |

## 12. Colour
| Subject | Status |
|---|---|
| RGB against YUV; 4:4:4 against 4:2:0; effect on text | partly (14) |
| Bit depth, HDR, colour management | open |

## 13. Remote display and screen sharing systems
| Subject | Status |
|---|---|
| RDP | partly (12) |
| VNC family (Tight, ZRLE, Hextile, TurboVNC, TigerVNC) | partly (18) |
| SPICE, PCoIP, Citrix HDX, NICE DCV, NoMachine, Chrome Remote Desktop | open |
| Game streaming: Parsec, Sunshine/Moonlight, GeForce Now, Steam Remote Play | open |
| Screen sharing: WebRTC, Zoom, Teams, Meet, Webex | partly (4) |

## 14. Recording, streaming and broadcasting products
| Subject | Status |
|---|---|
| Windows: Snipping Tool, Game Bar, Steps Recorder, Clipchamp | partly (21) |
| OBS, ShareX, ffmpeg, Bandicam, Camtasia, ScreenToGif, Loom, vMix | partly (22) |
| Streaming platforms' rules for screen content (YouTube, Twitch) | open |
| Test and automation recorders: Playwright, Cypress, Selenium, Puppeteer, Appium | partly (11) |
| Session replay products: rrweb, FullStory, LogRocket, Clarity | partly (19) |

## 15. Recorders built for AI
| Subject | Status |
|---|---|
| screenpipe, Microsoft Recall, Rewind/Limitless, open clones | partly (1, 2) |
| Agent run records: computer-use tools, OSWorld, browser agents | open |

## 16. Encoders and how a program reaches them
| Subject | Status |
|---|---|
| Software encoders for each codec: speed, CPU, RAM, screen modes | partly (24) |
| Hardware encoders: Intel, NVIDIA, AMD; what each PC has | partly (8, 14) |
| Windows interfaces: Media Foundation, Direct3D 11/12 video, WinRT transcoder | partly (16) |
| Other interfaces: vendor kits, Vulkan Video, ffmpeg libraries, WebCodecs | open |
| Start time of capture and encoders; PCs with two graphics processors | open |

## 17. Settings that decide size and quality
| Subject | Status |
|---|---|
| Quality mode against bitrate mode; key-frame distance; frame types | partly (5, 16) |
| Variable frame rate; skipping unchanged frames | partly (5) |
| Regions of interest; different quality per area | open |

## 18. Reading a recording back
| Subject | Status |
|---|---|
| Getting one exact moment quickly; indexes; decode cost | open |
| Serving the model: full frames, crops, difference pictures, timelines, text | open |

## 19. Files and storage
| Subject | Status |
|---|---|
| Containers: MP4, fragmented MP4, Matroska/WebM, MPEG-TS, AVI, zip, database, custom | open |
| Surviving a crash; appending safely | open |

## 20. Costs and limits
| Subject | Status |
|---|---|
| CPU, GPU, RAM, disk writes, battery for each approach | open |
| Losing nothing silently: dropped frames, timing, transient events | open |
| Licences and patents: what may be shipped | open |

## 21. How to judge the candidates
| Subject | Status |
|---|---|
| Test content: text, UI, scrolling, typing, video, animation, mixed | open |
| Quality measures: picture metrics, OCR accuracy, the model's own reading accuracy | open |
| A fair measurement of size, CPU and RAM on a real PC | open |
