# Deep Lite

An experimental focus-overlay fork of [Deep / Monocle](https://github.com/brycelewiswork/monocle)
by Bryce Lewis for Windows 10/11 x64. Upstream's MIT license is retained in [LICENSE](LICENSE).

## Ignored-app and focus-retention follow-up

A supplied Preview 4 diagnostic has `firefox.exe` matched by `settings.ignored_apps`.
This explains why those windows are kept sharp. The default list is empty; there
is no Firefox exception. The diagnostic cannot establish when or why that saved
entry was added. No exclusions are automatically removed by this update.

To apply the effect to Firefox, choose **Ignored apps → Firefox → Apply effect**.
The UI now spells out that Ignored apps always stay sharp. The quick-add button
explicitly says “Keep … sharp (exclude from effects)”. Live Blur and other saved
preferences are preserved.

Setup after settings/display changes now prioritizes the actual eligible foreground,
then the last real foreground, then a valid retained anchor, before z-order fallback.
An Ignored app being raised no longer steals the focus anchor during this rebuild.
Ignored-window caches refresh from a versioned settings snapshot, including during
rapid edits. Diagnostics show last real/tracked foreground, retained groups, recent
transitions, matching configuration and a per-window sharp reason.

Both supplied snapshots were taken with Deep Lite settings in the foreground.
Tablacus being sharp may therefore be retained prior focus; it is not yet proof
that Firefox's lift dragged Tablacus above the effect. [Evidence and limits](docs/ignored-focus-followup.md).

## Focus and desktop-input repair preview

New settings default to **Live blur**. Existing explicit Renderer choices are
preserved: if upgrading Preview 3, select **Renderer → Live blur** yourself.
For one global sharp group use Per-monitor focus OFF and App-wide focus OFF;
check Ignored apps for intentional exclusions. Defaults contain no ignored apps,
and there are no Firefox/Tablacus-specific exclusions.

All effect windows are now permanently noninteractive. Native clicks, drags,
double-clicks and title-bar gestures pass to the real apps/desktop; the previous
“first click only raises a blurred app” behavior has been removed. No synthetic
mouse events, icon moves or desktop-window reparenting are used.

Focusable tool windows now participate consistently in foreground tracking.
Nonactivating/small popups can receive the background effect without becoming
focus anchors. Separate windows sharing a hidden helper owner no longer become
one sharp group with App-wide OFF. Desktop shell windows are not app anchors.

**Always-on-top windows are still outside the effect's non-topmost band.** We do
not silently remove another app's topmost state or make the entire overlay always
on top. This may require a separate design if it matches the reported symptom.
Use **Show focus diagnostics** to inspect class, owner, styles, ignored status,
exclusion reason and actual root/GPU z-order. Reports omit window titles/URLs.
Firefox/Tablacus behavior has not been reproduced on the user's PC.

Static mask (Solid/Stripes/Grid) remains optional. It dims without real blur and
starts no WGC/D3D blur workers. DWM still composites either mode. No measured GPU
saving or GPU-zero claim is made. [Focus/input investigation and runtime checklist](docs/focus-input-repair.md).

## Renderer revision 2

The first preview was reported too heavy. This revision removes the normal
full-resolution input copy, caches completed blur images across fades, and uses
three reduced-resolution Ambient levels instead of eight native Gaussian levels.
Strong blur uses quarter-width/height processing on divisible dimensions; weak
and odd-size cases preserve the native path. Ambient keeps a native sharp base.
Display devices select their owning GPU, and monitor-specific operation counters
are written to `%TEMP%\deep-lite-gpu-blur.log` every 10 seconds.

The 20 Hz budget remains. WGC and final composition remain full resolution.
These are code-path reductions, **not a measured GPU-performance result**.
Ambient/tint appearance changes need visual checking. Fully hidden monitors are
not automatically suspended because opacity cannot safely be inferred from bounds.
See [counter interpretation, comparison procedure and limitations](docs/low-gpu-blur.md).

Deep's existing per-monitor/app-wide focus, ignored apps, desaturation, tint,
grain, shake toggle, fades and configurable hotkeys remain available.

## Download the isolated preview

Use a **Deep Lite** prerelease from [this fork's Releases](https://github.com/zufall-upon/monocle/releases).
Each release includes an unsigned x64 standalone exe, a ZIP with the license and
build information, an NSIS installer, SHA-256 sums, the exact source SHA and CI link.
Microsoft Edge WebView2 is required by the app. GPU/UI runtime remains untested.
The earlier unisolated `4a6c844` build is superseded and not recommended for trial.

Deep Lite does not automatically import or modify upstream Deep settings:

| Item | Deep Lite |
| --- | --- |
| Settings | `%APPDATA%\DeepLite\settings.json` |
| Logs | `%APPDATA%\DeepLite\deep-lite.log` |
| GPU diagnostics | `%TEMP%\deep-lite-gpu-blur.log` |
| Autostart Run value | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Deep Lite` |
| Installer identity | `io.github.zufallupon.deeplite`, product `Deep Lite` |
| Installed executable | `deep-lite.exe` |

Autostart defaults **OFF**; a fresh launch does not write or delete any Run value.
Opting in affects only the `Deep Lite` value, never upstream's `Deep` value. The ZIP
uses this same separate settings directory; it is not a per-folder portable profile.

The shared `Local\DeepSingleInstance` mutex intentionally prevents two overlays.
If Deep or Deep Lite is already running, a second launch exits before settings,
logging or autostart work. It does not stop the running app. When you choose to
switch versions, use the existing app's tray Quit command yourself. No user-PC
app is launched, stopped or overwritten by the build/verification workflow.

## Controls

Deep Lite starts disabled in the tray. The default hotkeys remain:

| Action | Shortcut |
| --- | --- |
| Toggle focus overlay | `Ctrl + Alt + Win + F` |
| Switch Deep / Ambient | `Ctrl + Alt + Win + M` |
| Show/hide settings | `Ctrl + Alt + Win + C` |

Hotkeys and shake sensitivity can be adjusted in settings. “Deep focus” still
names the uniform blur mode; “Ambient” is the progressive blur mode.

## Build and validation

Requires Node.js, Rust stable, Microsoft C++ Build Tools and the Windows SDK.

```sh
npm ci
npm run dev
npm run build -- --ci --bundles nsis -- --locked
```

The binary is `src-tauri/target/release/deep-lite.exe`; the installer is under
`src-tauri/target/release/bundle/nsis/`. Compilation does not launch the app.

The manual [Windows workflow](.github/workflows/verify-windows.yml) checks the
Windows code, requires at least 14 blur-policy, 8 isolation/settings, 18 focus and
4 renderer-lifecycle tests plus a native Win32 input-target fixture, runs the
library tests, builds/packages the exe and installer, and records source SHA,
Authenticode status and hashes. It uses a standard GitHub-hosted Windows runner
in this public repository. Releases link the successful run for their exact SHA.

The upstream `release.yml` remains in the repository; `v*` tags trigger it.
Deep Lite experimental prereleases use `deep-lite-preview-*` tags and verified
artifacts from the manual workflow instead.

The installer’s optional app-data deletion uses the Tauri bundle-ID directories.
It does not remove `%APPDATA%\DeepLite`; these settings and logs remain after
uninstall and can be removed manually if no longer needed.
