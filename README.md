# Deep Lite

An experimental low-GPU fork of [Deep / Monocle](https://github.com/brycelewiswork/monocle)
by Bryce Lewis. Keeps background blur, focus tracking, tint, grain and the settings
UI on Windows 10/11 x64. Upstream's MIT license is retained in [LICENSE](LICENSE).

**Windows compilation and policy tests do not establish GPU savings or runtime
quality.** Three-monitor behavior, visual quality and GPU/power measurements still
need validation. See [implementation and validation notes](docs/low-gpu-blur.md).

## What changed

- Budget background blur updates to 20 per second; reuse unchanged frames.
- Close capture after deactivation fades out; reopen on activation.
- Strong Deep blur uses half-width/half-height effect bitmaps. Weak blur and odd
  monitor dimensions use the original full-resolution path.
- Ambient keeps its sharp base and eight native-resolution blur bands.
- Rebuild monitor resources on display changes and resume.

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
| Autostart Run value | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\DeepLite` |
| Installer identity | `io.github.zufallupon.deeplite`, product `Deep Lite` |
| Installed executable | `deep-lite.exe` |

Autostart defaults **OFF**; a fresh launch does not write or delete any Run value.
Opting in affects only the `DeepLite` value, never upstream's `Deep` value. The ZIP
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
Windows code, requires at least 10 blur-policy and 6 isolation tests, runs the
library tests, builds/packages the exe and installer, and records source SHA,
Authenticode status and hashes. It uses a standard GitHub-hosted Windows runner
in this public repository. Releases link the successful run for their exact SHA.

The upstream `release.yml` remains in the repository; `v*` tags trigger it.
Deep Lite experimental prereleases use `deep-lite-preview-*` tags and verified
artifacts from the manual workflow instead.
