# Preview 4 diagnostic follow-up

## Evidence, without inferring user intent

Two supplied reports have Per-monitor OFF, App-wide OFF, Live blur selected and
Deep Lite's own settings window as the actual foreground. Normal Firefox windows
are eligible, non-topmost and `ignored=true`; the Tablacus window is eligible,
non-topmost and `ignored=false`. Both are above the root and all three GPU windows.
Other ordinary apps are below the effect. A Firefox dialog is separately topmost.

`is_ignored_hwnd` resolves a window's lowercased executable basename and exactly
matches the normalized runtime list supplied by `AppSettings.ignored_apps`.
Defaults have an empty list. Settings load only the fork's settings directory,
with no upstream migration. The existing quick-add card and Ignored-app picker
append entries through update_settings; the remove control deletes the selected
entry. There is no automatic Firefox or Tablacus exclusion. Both Firefox processes
match the same saved executable name, regardless of Per-monitor/App-wide options.

Thus the Firefox exclusion is established, but the intent, original addition time
and original UI interaction are not. Existing records do not establish that history.
No setting is silently deleted. Future update_settings changes log old/new normalized
excluded names, without claiming historical provenance. To apply the effect, remove
Firefox using **Ignored apps → Apply effect**. A separate topmost dialog remains
outside the ordinary non-topmost overlay band even after removing its exclusion.

Both reports have settings foreground, so they do not show the transition being
reported. Tablacus may legitimately be the retained last real focused group.
The reports alone cannot establish that Firefox's raise lifted it or that a recent
Firefox foreground transition was processed. New diagnostics expose that missing
state: last observed real foreground, last processed foreground, retained anchors
and sharp members, a bounded transition history, the exact normalized ignored list
and its revision, and each window's sharp reason. All rows match the same captured ignored-list snapshot. Window/z-order and tracker state are still sequential observations, not an atomic desktop snapshot; recapture after transitions settle. No titles/URLs are added.

## Confirmed code defects and changes

1. Initial/rebuild setup cleared all tracked anchors and selected z-order's first
   eligible window. Ignored windows had just been raised without activation, so
   their artificial rank could replace the actual app. Setup now prefers current
   eligible foreground, then revalidated last real foreground, then valid existing
   anchors. First-time/no-valid-anchor fallback still uses eligible z-order.
2. Setup stored settings/shell foreground as `last_fg`, discarding the actual app
   identity. Observation of real apps is now distinct from processed foreground,
   and works while inactive too. Settings focus cannot overwrite the real-app value.
3. An ignore edit requested immediate setup but the HWND cache could still describe
   the old list. The tracker snapshots normalized entries and revision under one
   lock, refreshes for setup or revision changes, and acknowledges only the revision
   it enumerated. Edits during enumeration remain pending for the next iteration.

Input passthrough from Preview 4 is unchanged: layered, transparent, nonactivating
visuals, with no input catcher, synthetic mouse events or user-desktop modifications.

## Tests and remaining boundary

Regression policies cover ignored-app raise vs actual foreground, ignore add/remove
while settings is foreground, display changes, monitor retention/transfer, invalid
last anchors after minimize/close, concurrent ignored-list updates, and exact saved
exclusion matching with no defaults. The existing native Win32 fixture still checks
opaque DirectComposition input passthrough and now also checks that raising an
unrelated ignored fixture with NOACTIVATE leaves the background fixture below its
divider and does not change foreground.

These fixtures are not Firefox/Tablacus or a real Explorer desktop. Runtime effects,
physical icon gestures, three-monitor transitions and GPU costs still need user-side
verification. If a symptom remains, capture the new report after switching between
an ordinary app and Tablacus/Firefox; retained state avoids losing the relevant
foreground identity when settings opens. No user-PC process or settings were changed
while preparing this follow-up.
