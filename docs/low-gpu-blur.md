# Low-GPU blur experiment

This fork keeps Deep and Ambient blur, tint, desaturation, grain, the existing
settings UI and window focus tracker. It is an experimental source build, not a
measured or Windows-validated release. Upstream: brycelewiswork/monocle,
base commit `a1060b30c40f127b9796d7aed7eb3b9c4f6f7692` (MIT, Bryce Lewis).

## What changed

- Each monitor consumes at most one newest queued frame on a 50 ms Win32 timer.
  CopyResource, the effect chain and Present run at most once per timer tick.
  The pool drain is bounded to its two buffers, avoiding producer starvation.
- Request the same 50 ms minimum WGC update interval where the OS supports
  IGraphicsCaptureSession5. Older systems fall back to consumer throttling.
- After deactivation finishes fading, close the capture session. Recreate it on
  activation. Also suspend when blur, desaturation and GPU tint are all off.
- Reuse the last capture for settings/mode/fade changes; skip drawing/Present
  when neither the capture nor shared state has changed. A resumed window stays
  hidden until a fresh frame has been presented.
- Monitor removal, addition, geometry changes and resume recreate per-monitor
  resources. Failed workers retry on the next one-second reconciliation pass.
  Validate capture dimensions before copying, and recover from COM/draw errors.
- Resize the existing overlay/input/grain layers on display-change broadcasts;
  release replaced grain bitmaps/DCs and refresh the focus tracker.

The 50 ms timer is an upper-rate budget, not a guaranteed frame rate or latency.
The focus tracker remains at its existing rate. Blur animation/background video
can look less smooth at 20 updates/s. Static desktop savings depend on whether
WGC supplies new frames; a repeated identical frame is not pixel-compared.

## Half-size Deep blur

Strong Deep blur now has a separate half-width, half-height path. The original
saturation/tint graph is first materialized at native size and 96 DPI. This
preserves the pixel scale of the existing DISSOLVE tint (despite old upstream
comments describing it as an HSL Color blend). A second bitmap receives a 2:1
linear downsample. Gaussian blur is then rendered into a third, half-size bitmap
with half the original sigma. That completed bitmap is upscaled for composition.
Every bitmap and the context use 96 DPI, so sigma corresponds to physical pixels;
OS display scaling must not be applied a second time.

The quality policy keeps the native Deep path below sigma 8 (10% of STDDEV_MAX),
for odd or smaller-than-two-pixel dimensions, and for non-finite parameters. The
odd-size fallback avoids rounding, cropping or unequal X/Y scale factors. The
threshold is conservative but not visually validated; changing across it needs
an image-quality check. Allocation failure also falls back to native Deep until
the worker is recreated. Offscreen drawing/device failures still trigger worker
recovery rather than marking an unfinished frame as presented.

Ambient is unchanged: its sharp base, eight Gaussian bands and masks remain at
native resolution. During a mode crossfade, only the separate Deep contribution
uses the half-size path. Fade opacity is applied after upscaling, as before.

The three extra bitmaps are allocated lazily when the half-size path is first
used, reused until monitor-worker teardown, and remain allocated while inactive
or in Ambient. They total 1.5 native BGRA surfaces: about 47.5 MiB per 3840x2160
monitor, or 142.4 MiB for three such monitors, excluding driver padding and other
resources. Three offscreen passes also add work. This is a memory/processing
tradeoff; no net speedup has been measured. A future cache of completed blur
results could avoid repeating these passes during fade-only updates.

## What did not change

WGC capture, CopyResource, final swapchain output and desktop composition remain
full resolution. A smaller WGC frame-pool size would clip content, not downsample
it. Only the eligible Deep Gaussian branch gets an explicit quarter-pixel-count
working surface. Direct2D's default Gaussian optimization already performs
internal pre-scaling in some cases, so that pixel ratio is not an estimate of
overall GPU savings. Ambient's eight-band cost remains.

The expected reduction is in repeated capture consumption, copies, effects and
Present calls. WGC's internal work depends on OS support and the compositor.
A 20 fps consumer does **not** imply a proportional reduction in total GPU load.
No GPU utilization, power, VRAM or latency measurements have been obtained.

## Validation status

On the Ubuntu editing host: source/API review and `git diff --check` passed.
Independent reviews covered recovery/topology and the subsequent half-size
Deep path. The code adds error propagation, worker retries, fresh-frame show
gating and overlay resizing. The half-size review checked target/input separation,
pass boundaries, native Tint/Ambient preservation, fallback paths, and the binding
between pure policy and the Windows renderer. API signatures were checked against
windows 0.61.3/windows-core 0.61.2 source. The null-frame conversion is compared with `Error::empty().code()`, not
an assumed E_POINTER HRESULT.

Rust/MSVC/Windows SDK are not installed on this host. New toolchain/binary
installation and executing the app require approval. No app process was stopped,
installed, overwritten or launched. The manual Windows build workflow is prepared
but has not been run. A successful future build would not establish visual or GPU
correctness.

`src-tauri/src/blur_policy.rs` now contains 10 unit tests, **prepared but not run**.
They cover static-frame reuse and settings updates; missing/failed/overtaken
presentation; fade-out and fresh-frame restart; rapid toggling; zero-blur with
other effects; content/texture resize rejection; independent monitor state;
physical sigma and exact half dimensions; weak/odd/invalid quality fallback; and
Deep-versus-Ambient branch selection. These functions are used by the Windows
renderer, rather than being a separate simulation of its intended behavior.
The manual workflow rejects discovery of fewer than these 10 policy tests before
running the library tests. Passing them would validate policy only, not WGC,
Direct2D execution, 20 Hz timing, visual quality or hardware lifecycle.

## Windows acceptance matrix — all runtime cases pending

Use a separately approved test machine/session. Do not replace or run alongside
an existing Deep installation: this fork retains upstream app/config identities,
window classes and hotkeys. Keep the existing settings and installation intact.

| Case | Expected check |
| --- | --- |
| Start disabled, three monitors playing video | No WGC sessions until activation; no copy/effects/Present while idle |
| Activate, deactivate, rapid toggles, zero/long fade | Fade completes; capture closes after fade-out; next activation uses fresh content |
| Static desktop, then move a background window | No redundant Present on ticks with no frame or state change; new frames appear |
| Change blur/tint/mono on static content | Cached capture redraws without requiring desktop movement |
| Deep sigma just below/at/above 8, Tint off/on, mono | Native/half path transition has acceptable appearance; DISSOLVE grain scale stays stable |
| Deep on even/odd dimensions and 100%/150%/200% display scaling | Exact coverage, no doubled sigma scaling, odd sizes use native fallback |
| Deep→Ambient→Deep and crossfade | Ambient top remains sharp; no half-size target/graph leaks into its eight bands |
| Half-size bitmap allocation failure | Native Deep continues without a per-frame allocation retry loop |
| All GPU effects zero; restore blur | GPU window hides and capture closes; blur resumes correctly |
| Deep ↔ Ambient including mid-transition toggles | Both treatments and tint/grain remain correct; no blank/stuck frame |
| Window drag across screens; per-monitor/app-wide focus | Crisp focus group and original z-order/input behavior remain usable |
| 3→2→3 screens, rotation, resolution/DPI/origin changes | No stale worker, mismatched copy, gap in grain/input layers, or sustained resource growth |
| Sleep/resume, lock/unlock, capture/device error | Fresh capture returns; no permanent frozen image or unbounded error log |
| Windows lacking MinUpdateInterval support | Capture still starts; the consumer budget remains effective |

Compare upstream and fork separately with identical settings, resolution, refresh
rate, video/application workload, power plan and driver. For Deep and Ambient,
record at least 60 s after warm-up in disabled/static/moving/video states. Record
whole-system and per-process GPU engines (3D/Copy), frame/present counts, CPU,
dedicated/shared GPU memory, power if available, and perceived latency. Repeat
three times, reporting medians/ranges rather than a single peak. Keep the Windows
build result, runtime pass/fail matrix and measured savings as separate evidence.

For the new path compare upstream, the 20 Hz commit `0689690`, and the half-size
Deep revision separately. Include sigma 0/4/7.99/8/24/80, Tint off/on and Deep,
Ambient and mode transitions. Record offscreen pass costs and the additional GPU
memory as well as end-to-end results. Useful references: [WGC frame sizing and
lifetime](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
and [Direct2D Gaussian optimization](https://learn.microsoft.com/en-us/windows/win32/direct2d/gaussian-blur).
