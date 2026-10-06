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

## What did not change

Capture textures and D2D processing remain full resolution. There is no WGC
resolution reduction, downstream downsampling, or reduced Ambient band count in
this patch. Ambient still evaluates eight blur bands. This first patch avoids
changing its sharp-to-blurred appearance. A materialized low-resolution effect
surface is a possible next experiment after the baseline has been measured.

The expected reduction is in repeated capture consumption, copies, effects and
Present calls. WGC's internal work depends on OS support and the compositor.
A 20 fps consumer does **not** imply a proportional reduction in total GPU load.
No GPU utilization, power, VRAM or latency measurements have been obtained.

## Validation status

On the Ubuntu editing host: source/API review and `git diff --check` passed.
An independent reviewer identified recovery and topology issues; the final code
adds error propagation, worker retries, fresh-frame show gating and overlay
resizing. API signatures were checked against windows 0.61.3/windows-core 0.61.2
source. The null-frame conversion is compared with `Error::empty().code()`, not
an assumed E_POINTER HRESULT.

Rust/MSVC/Windows SDK are not installed on this host. New toolchain/binary
installation and executing the app require approval. No app process was stopped,
installed, overwritten or launched. The manual Windows build workflow is prepared
but has not been run. A successful future build would not establish visual or GPU
correctness.

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
