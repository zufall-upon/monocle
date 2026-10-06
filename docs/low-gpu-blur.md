# Deep Lite renderer revision 2

The first preview (`a6846a5`) was reported still too heavy by the user. Its successful build did not establish acceptable performance. This revision changes the rendering work, not just the 20 Hz update budget. No GPU percentage reduction is claimed.

## Cost audit and changes

| Path | First preview | Revision 2 |
| --- | --- | --- |
| D3D adapter | Default hardware adapter for every monitor | Match DXGI output HMONITOR to its adapter; log actual adapter/name/LUID and fallback |
| Deep capture ingestion | Full-resolution CopyResource, native tint materialization, downsample | Direct WGC surface sampling into an owned working bitmap; compatibility copy only if direct wrapping fails |
| Strong blur | Half width/height for Deep only | Quarter width/height for both modes when sigma >= 8 and dimensions divide by 4; half for even non-multiples of 4; odd/weak cases stay native |
| Ambient | Native sharp base plus 8 native Gaussian levels | Native sharp base plus 3 cached Gaussian levels on the working surface |
| Fade/mode transition | Live Gaussian evaluation on each draw | Composite completed bitmap caches; build only missing levels |
| Focus movement | Globally invalidates drawing despite rectangles not being read | Geometry-only notification does not invalidate GPU pixels |
| Disabled | Capture stopped, intermediate images retained | Capture stopped and working caches released after fade |
| Retired overlay anchors | 10 transparent full-desktop windows | 1 transparent z-order anchor |

Effect cache identity contains captured-frame generation, sigma, desaturation and tint. Fade and mode mix are not part of that identity. Deep uses the highest level shared with Ambient; entering Ambient adds only missing lower levels and its sharp base. Ambient with zero sigma has no Gaussian passes. Identical settings do not change the effect cache.

WGC still captures at native monitor resolution. This is not a claim of smaller WGC capture or guaranteed removal of cross-adapter traffic. Direct2D's own Gaussian optimization may already pre-scale. Final swapchain/composition is native resolution. Capture consumption still has a 50 ms upper-rate budget; this is not the principal revision-2 change.

## Lifetime and compatibility

WGC frames are released after source ingestion and EndDraw within the render tick. Effects are disconnected from borrowed input even on failure. Owned reduced input and completed results survive for subsequent composition. Ambient additionally caches native raw/processed sharp images. Entering Ambient or changing processing size without a current frame restarts capture to obtain a fresh native source; the existing displayed frame remains until that arrives.

If a driver cannot wrap the WGC surface as a D2D bitmap, a lazy owned native texture with CopyResource is used as a compatibility fallback. Its calls are counted. Draw/device errors still recreate the worker. Monitor topology/size changes recreate all resources. No OS GPU assignment is changed.

## Deliberate limitations

- Three Ambient levels change the progressive blur appearance. Tint DISSOLVE is evaluated at working resolution for blurred levels, so its texture can change. Ambient's sharp base remains native; separate film grain is retained.
- Exact integer scaling preserves edge coverage and physical sigma; odd dimensions and weak blur retain the more expensive native path. DPI does not multiply sigma because all D2D bitmaps/contexts use 96 DPI.
- No incoming frame and unchanged settings means no ingestion, Gaussian or Present. Fade-only updates composite cached results without Gaussian work. New WGC frames are conservatively treated as changed; repeated identical pixels with different frames are not detected by a CPU readback/hash.
- Empty dirty-region metadata is **not** used to skip work: a throttled/dropping frame pool does not provide a proven cumulative-damage guarantee here.
- Disabled/faded-out capture sessions stop. Fully occluded monitors are **not automatically suspended**: ordinary window style/rectangle tests cannot prove opacity (DWM glass and transparent composition exist). A proposed heuristic was removed during review to avoid disappearing backgrounds. Achieving zero capture for every hidden monitor remains unresolved.
- Multiple monitor devices remain separate, now selected by owning output. Ambient's native caches add memory. Neither memory pressure nor performance on the user's mixed GPUs has been measured.

## Counters and the minimum real-machine comparison

`%TEMP%\deep-lite-gpu-blur.log` records actual adapter identity, optional API support and cumulative counters every 10 seconds per monitor:

- `capture`: frames acquired; `skipped`: older queued frames discarded.
- `copy_resource`: compatibility full-texture copies (normally zero on the direct path).
- `downsample`: owned working-input draws (native on weak/odd fallback).
- `native_ingest`: native Ambient/zero-blur input-cache copies.
- `sharp`: native sharp effect evaluations; `blur`: Gaussian bitmap evaluations.
- `present`: swapchain calls; `starts`/`stops` and `session`: capture lifecycle.

Take differences between two log lines for the same monitor and worker, dividing by elapsed milliseconds. These are CPU-side submitted operation counts, **not GPU duration/utilization measurements**. New workers reset counters. Logs contain geometry/adapter names but no captured pixels or application content.

For an authorized test, record only: preview SHA, mode/blur/tint settings, each display's resolution/scaling and logged adapter; 30 seconds disabled, 30 seconds static enabled, and 30 seconds of the same moving content in Deep then Ambient. Pair log deltas with Task Manager's per-process GPU/GPU-engine columns for Deep Lite and Desktop Window Manager. Do not attribute whole-adapter GPU percentages solely to this app. No user-machine measurement or executable launch is performed by CI.

## Verification boundaries

Pure Rust tests exercise source/draw lifecycle, cached effects across repeated composition, mode-level reuse, color invalidation, exact scaling/weak/odd/rotated extents and a deterministic three-monitor operation-count scenario. Isolation tests preserve separate settings/logs/Run identity and the shared single-instance mutex. Windows CI compiles, runs tests and packages; it does not exercise WGC, adapters, D2D, visual quality, occlusion or real GPU performance.

Pending real-machine regression matrix: Deep/Ambient and interrupted crossfade; ON/OFF/rapid toggle; sigma below/at/above 8; tint/mono; 100/150/200% DPI; odd sizes and screen edges; 3→2→3 displays and rotation; sleep/resume/device errors; mixed-adapter displays; direct-wrap fallback; fresh frame on native-source re-entry. Source reviews and CI evidence are linked in the release for its exact commit.

## Isolation and license

Settings/logs remain `%APPDATA%\DeepLite`; no old Deep configuration import. Autostart defaults OFF and uses Run value `Deep Lite`, matching installer cleanup. Product/binary/identifier remain Deep Lite / deep-lite.exe / io.github.zufallupon.deeplite. Shared mutex prevents simultaneous overlays without stopping the current app. Uninstall retains the settings/log directory. Upstream MIT copyright Bryce Lewis is preserved.
