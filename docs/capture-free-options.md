# Capture-free direction and focus investigation

The user reports both GPU previews remain too heavy. Whole-adapter screenshots (51%, then 90% on GPU0, different GPU1 values) establish neither equivalent conditions nor Deep Lite's share of that work. The usability report is still a failure of the performance goal. This document compares a different approach; it does not claim another measured speedup.

## What PrivacyScreen actually does

Reviewed [dhr412/privacyscreen at d24ae05](https://github.com/dhr412/privacyscreen/blob/d24ae05bf4155129e6ce8c356eee328c17a20998/src/privscrn.zig): create one layered click-through window per monitor, calculate an alpha-only vignette or repeating mask bitmap, submit it with UpdateLayeredWindow, then wait on an event. It does not sample underlying application pixels or run a realtime blur. No code was copied or executed for this comparison.

| Approach | Appearance | Continuous application work | Conditions |
| --- | --- | --- | --- |
| Static tint/pattern (recommended lightweight mode) | Dim, stripes, checkerboard, vignette; underlying sharp detail may remain perceptible | No capture, copy or Gaussian loop. Regenerate only for settings/display changes; update focus coverage only on events/geometry changes | Normal Win32 layered windows; DWM still composites them |
| System Desktop Acrylic | OS-generated frosted-glass backdrop | No application WGC loop; DWM performs the effect | Documented DWM system backdrop requires Windows 11 build 22621+. Full-screen, multi-monitor, nonactivating overlay behavior and cost need a separate test |
| Current WGC blur | Actual live captured content is blurred, including adjustable Deep/Ambient modes | Native WGC plus own image processing/presentation remains | Two preview revisions failed the user's performance goal |

[Mica](https://learn.microsoft.com/en-us/windows/apps/design/style/mica) is an opaque wallpaper-derived material, not live background-window blur. [DWM_SYSTEMBACKDROP_TYPE](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type) distinguishes Mica and Desktop Acrylic and documents the Windows build requirement. [System backdrop guidance](https://learn.microsoft.com/en-us/windows/apps/develop/ui/system-backdrops) targets Acrylic at transient UI; it does not guarantee an always-on three-screen overlay will be cheap. The old [DwmEnableBlurBehindWindow](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmenableblurbehindwindow) no longer produces blur starting with Windows 8. Undocumented accent-policy tricks are not a reliable cross-version replacement.

GPU-zero is not promised: [DWM](https://learn.microsoft.com/en-us/windows/win32/dwm/dwm-overview) still composes the desktop. Moving continuous blur to CPU would merely change the bottleneck.

## Implemented with the user's explicit approval

Static mask is now the default, including missing renderer fields in earlier fork settings. Solid, Stripes and Grid use the existing virtual-desktop tint/input window and a GDI solid/hatch brush. Windows caches the painted content. This is not a privacy/security screen: sharp detail can remain perceptible. Tint controls opacity/color. Live blur remains selectable; its blur, grain and mono preferences are retained but their controls are disabled in Static mode.

Static never creates the monitor GPU workers or their WGC/D3D/D2D/swap-chain resources. Switching out of Live wakes the supervisor, which stops/joins workers and releases them. Live OFF releases workers once fading ends. Returning to Live re-enumerates current monitor geometry and starts fresh resources. Grain surfaces are allocated only for Live and released on switching to Static; Static skips fullscreen UpdateLayeredWindow uploads. No CPU blur replaces the GPU work. Static painting occurs on repaint/settings/display changes, with alpha changes during fades. The inherited focus tracker still polls; maintenance runs at most every 250 ms. This is not PrivacyScreen's identical event-only implementation.

The GPU log records `renderer=static gpu_workers=0` after teardown. It is a resource-state diagnostic, not a measurement of DWM or total GPU usage. No user-machine runtime test has been performed.

### Windows runtime checklist (pending)

- Start with fresh/previous settings; confirm Static selected, tint visible and no GPU workers/capture; adjust Solid/Stripes/Grid and tint.
- Switch Static → Live (Deep/Ambient) → Static while active and during a fade; confirm old grain/blur disappears and workers return to zero.
- Repeated ON/OFF and resume from sleep; reconnect/remove/resize/rotate a monitor in both modes, including while OFF; verify coverage, input and fresh Live capture.
- Browser → another app on the same monitor, Alt-Tab, browser self-raise without activation, minimize/restore/close, popups, monitor transfer and settings/tray activation.
- Compare per-monitor ON/OFF, App-wide ON/OFF, Ignored apps; each intentionally changes the sharp set.
- Compare identical 3-monitor workloads OFF vs Static vs Live. Measure Deep Lite and dwm.exe GPU engines separately, CPU/power and both adapters, rather than attributing whole-adapter readings to one process.

Policy tests cover renderer lifecycle, settings migration, focus snapshots and prior blur scheduling. They do not execute Win32 focus, DWM, WGC or browser integration.

## Browser remains sharp: settings versus defects

Current sharp regions come from real app windows being raised above the overlay, not transparent image holes. FOCUS_RECTS are not used as GPU image inputs. Therefore stale Gaussian cache is not evidence of a hole remaining open.

- Per-monitor focus defaults ON: browser on monitor A remains sharp when focus moves to monitor B. Turn it off for a single global focused group.
- App-wide focus defaults OFF: when enabled, same-process browser windows join the sharp group.
- Ignored apps deliberately remain sharp. Settings/tray/transient and topmost foreground windows preserve the preceding group.
- Confirmed defect: the unchanged-foreground early return skipped lifecycle and z-order reconciliation. A background browser raising itself without activation, or a nonforeground monitor's anchor closing/minimizing, could leave an obsolete sharp set.
- Confirmed defect: minimized windows were not explicitly excluded, and the first enumerated group member was treated as its anchor even if it was a popup.

The repair reevaluates available anchors/membership at most every 250 ms before the unchanged-foreground early return, excludes minimized windows, makes the actual anchor explicit at group index zero, and only changes z-order when a window violates the intended sharp set. Normal foreground transitions remain immediate on the tracker tick. Tests cover same/other-monitor focus and Alt-Tab policy, close/hide/minimize/restore, nonactivating self-raise, explicit ignored/app-wide membership, popup order and monitor transfers. These tests do not execute browser/Win32 window operations; runtime verification remains pending.

## Capture feedback and rate audit

GPU draw has one entry point: WM_TIMER. There is no FrameArrived render callback. Added elapsed-time gating supplements the 50 ms timer to prevent catch-up rendering bursts. Counters remain per-monitor: Ambient can submit three Gaussian passes per accepted frame, and DWM/WGC internal work is outside this budget.

Both previous previews called SetWindowDisplayAffinity but ignored the result. Layered overlays now require successful exclusion and readback before permitting capture. The non-layered DComp window requires successful Set; Get is diagnostic because [GetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowdisplayaffinity) documents layered-window conditions. Successful readback with a mismatching value rejects capture. Settings protection requests are logged as well. This detects a feedback risk; it does not prove feedback caused the user's GPU readings. [WDA_EXCLUDEFROMCAPTURE](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity) requires Windows 10 version 2004+ for full exclusion behavior.

The retired Magnification host was still created at startup but had no active refresh timer because nothing enabled it. Its initialization has now been removed; it is not presented as the identified high-load cause. No OS GPU assignment, billing setting, or user-machine executable was changed.
