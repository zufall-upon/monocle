# Deep Lite

An experimental focus-overlay fork of [Deep / Monocle](https://github.com/brycelewiswork/monocle)
by Bryce Lewis for Windows 10/11 x64. Upstream's MIT license is retained in [LICENSE](LICENSE).

## Capture-free preview

**Static mask is now the default**, including when upgrading old Deep Lite settings.
Choose Solid, Stripes or Grid; Tint controls its color and opacity. This dims/masks
background apps rather than blurring their pixels. Live blur is still selectable
under Renderer, with previous blur preferences preserved.

Static mode creates no WGC sessions or D3D/Direct2D blur workers, performs no
screen copies, Gaussian passes or swap-chain Presents, and allocates no grain
bitmap. Windows caches the GDI overlay fill; settings/display changes repaint it
and fades change its alpha. Focus tracking still runs; DWM still composites the
windows. **GPU-zero and a measured speedup are not claimed.**

Switching to Static tears down live GPU workers; disabling Live blur tears them
down after fade-out. Switching back creates fresh resources for current monitors.
The browser-focus repair reconciles stale window membership/z-order every 250 ms.
Per-monitor focus intentionally keeps a sharp window on each monitor: turn it OFF
for only one global focused group. App-wide focus and Ignored apps also expand
what stays sharp. See [investigation and runtime checklist](docs/capture-free-options.md).

Windows compilation and policy tests do not establish actual GPU/power savings,
visual quality or real three-monitor behavior. Runtime validation remains pending.

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
Windows code, requires at least 14 blur-policy, 7 isolation/settings, 6 focus and
4 renderer-lifecycle tests, runs the
library tests, builds/packages the exe and installer, and records source SHA,
Authenticode status and hashes. It uses a standard GitHub-hosted Windows runner
in this public repository. Releases link the successful run for their exact SHA.

The upstream `release.yml` remains in the repository; `v*` tags trigger it.
Deep Lite experimental prereleases use `deep-lite-preview-*` tags and verified
artifacts from the manual workflow instead.

The installer’s optional app-data deletion uses the Tauri bundle-ID directories.
It does not remove `%APPDATA%\DeepLite`; these settings and logs remain after
uninstall and can be removed manually if no longer needed.
