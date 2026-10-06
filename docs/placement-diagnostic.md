# Placement diagnostic preview

This is a diagnostic build. The actual Firefox/Tablacus symptom is not confirmed
fixed. No settings reset is required. Existing preferences are preserved.

## Short real-PC trial

Use the same settings that produced the issue. Activate the effect, switch from
Firefox to Tablacus (or another ordinary app), wait 5 seconds, and open **Show
focus diagnostics**. Copy that report including its `placement_diagnostic=1`
header and `placement#` samples. There is no need to repeatedly reset settings,
change Firefox profiles or install a non-portable browser.

Each sampled demotion records its source, target/root/insertion HWND, PID/TID,
caller TID, target/current process integrity RID and UIAccess (read-only; an
unavailable HRESULT is recorded if access fails), API result, and positions:

- `before`: just before SetWindowPos.
- `after`: immediately after it returns.
- `next`: next tracker iteration, before our next GPU pin and reconciliation.

The last observation follows other operations from the previous iteration. A
changed position does not prove Firefox itself raised the window. Concurrent
OS/app changes remain possible between observations. Destroyed handles or changed
PID/TID are marked; same-thread HWND reuse cannot be ruled out. Windows desktop
observations are sequential, not atomic.

Sampling is at most 4 attempts/second globally and 1 per 2 seconds for each HWND.
The report keeps 16 completed samples, plus at most 4 pending observations. The
normal log receives at most 120 sample lines per process lifetime. Further samples
continue in memory. All API attempts/errors are counted even when not sampled; excluded/ineligible
early returns are counted separately.
No titles, URLs or command lines are added; focus labels in new logs now contain
only HWND/executable name. Existing log files may retain titles from older builds.

## Separate owner repair

Background demotions use SWP_NOOWNERZORDER, both during initial setup and later
focus/reconciliation. This prevents a shared hidden owner from dragging a sharp
sibling below the blur in the native fixture. Root/GPU positioning and sharp
promotion are unchanged. The native comparison calls the same production helper
and preserves its unmodified baseline as the control. This independently reproduced
owner defect is not claimed as the cause of the user's ownerless Firefox window.

## Validation boundaries

Native tests cover the three-GPU-style HWND ordering model, hidden shared owner,
actual ignored-app/settings foreground transitions and repeated reconciliation.
A separate worker thread vetoes WM_WINDOWPOSCHANGING z-order changes: SetWindowPos
returns success but the position remains unchanged; removing that veto lets the
same call move the window. A second target is successfully lowered and then
raised before the next observation; the report must distinguish its immediate
below-root result from its later above-root position. The fixture exercises the
production trace and helper.
The existing opaque DirectComposition input fixture remains required.

These are synthetic windows in the Windows CI runner, not Firefox, Tablacus or
Explorer. Cross-process/different-integrity placement, physical three-monitor
operation, GPU load and actual icon gestures are not verified. Token information
is only queried; this build does not elevate or modify process privileges.
