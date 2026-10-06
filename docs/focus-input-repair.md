# Focus and desktop-input repair

The user reports Per-monitor OFF, Codex affected, Firefox/Tablacus not affected,
and desktop icons not clickable. Live blur is required. These are runtime reports;
we have not reproduced the user's exact styles/owner chains on their PC.

## Confirmed code defects addressed

- The tint window deliberately consumed mouse-down/up/double-click messages. Its
  passthrough heuristic only recognized app nonclient borders, not desktop icon
  controls. Empty/desktop hit areas could therefore swallow native gestures.
  Removed the input catcher and its hit-testing/polling/forced foreground calls.
  All visual windows share disabled/nonactivating/transparent styles. Layered
  windows use permanent WS_EX_TRANSPARENT. DComp windows are also WS_DISABLED;
  HTTRANSPARENT is only a fallback, not assumed to route cross-thread input alone.
- TOOLWINDOW was excluded by reconciliation but allowed by foreground handling.
  Now one classification function drives both. Background-effect eligibility is
  separate from focus-anchor eligibility: nonactivating and small app popups can
  be covered without becoming the active group. Desktop/menu/system helper
  classes and all our own process windows remain excluded.
- Comparing ultimate root owners grouped independent windows sharing an invisible
  helper. The focus family now stops at hidden/cloaked/minimized/shell owners,
  while visible modal/owned dialog families remain together.
- Sharp regions are ordinary windows above the visual stack. Legacy focus-bound
  rectangles do not make GPU image holes. No process-specific exception exists.
  Defaults have no Ignored apps; explicit user exclusions remain unchanged.

## What is not established

Topmost windows are deliberately kept in their original band and remain outside
this non-topmost overlay. Removing the skip then demoting them would change both
owned/owner topmost state according to SetWindowPos semantics. We did not do that.
Their actual styles, App-wide and Ignored settings must be checked before claiming
this explains the user's Firefox/Tablacus result. The new diagnostic report includes
rank, HWND, process/executable, class, owner/family, tool/noactivate/topmost flags,
anchor status, exclusion reason, ignored status, and root/GPU-relative ordering.
It does not include titles, URLs, screen pixels or full executable paths.

The user suspects iGPU selection explains high load. That is not a performance
measurement or resolution. Live blur still uses WGC and GPU effects.

## Defaults and compatibility

New/missing Renderer settings default to Live blur. An explicit `mask` selection
is preserved, as are all other saved preferences. Preview 3 upgraders must choose
Renderer → Live blur. For one global sharp group choose Per-monitor OFF, App-wide
OFF, and check Ignored apps. Static remains optional. Existing running apps and
upstream settings/executable are not touched.

## Verification boundary

Automated: classification of visible tool windows and nonactivating small popups;
hidden shared owners vs visible modal chains; topmost exclusion without style
mutation; global foreground replacement; previous minimize/restore, monitor transfer,
mode/fade/capture policies; settings migration/roundtrip. The native Windows fixture
creates its own normal window plus visual windows using the production style
functions and verifies WindowFromPoint still returns the underlying fixture across
show/hide and recreation. It does not send real mouse input, render WGC frames,
exercise Explorer's listview, or execute Firefox/Tablacus.

Pending on a real Windows desktop (not performed on the user's PC):

| Scenario | Required check |
| --- | --- |
| Desktop icons | Single click selection, double-click open, drag/marquee, context menu; no icon movement except the user's own drag |
| Taskbar/Start | Click, context menu, launch, app switching in Live and Static |
| App windows | Firefox/Tablacus/Codex transitions with Per-monitor OFF/App-wide OFF; native first click, title-bar drag, resize, close |
| Keyboard | Alt-Tab, hotkeys, settings/tray focus preserves last real group |
| Lifecycle | Minimize, restore, close, popup/dialog open/close, reconnect/rotate/remove monitors, sleep/resume |
| Modes | ON/OFF during clicks and fades; Live/Static switch; Deep/Ambient; no retained input blocker |

## Platform references

- [Layered-window hit testing](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features#layered-windows)
- [EnableWindow / disabled windows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enablewindow)
- [WindowFromPoint skips disabled windows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-windowfrompoint)
- [SetWindowPos / owner and topmost semantics](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos)
