# Deep Lite

A Windows 10/11 x64 focus-overlay fork of [Deep / Monocle](https://github.com/brycelewiswork/monocle)
by Bryce Lewis. Upstream's MIT copyright and license are retained in [LICENSE](LICENSE).

## Download and update

**[Deep Lite 0.1.1](https://github.com/zufall-upon/monocle/releases/tag/deep-lite-v0.1.1)**
is the first stable release, with the same overlay behavior as Preview 7
(`5da199958507`). The user reported that Preview 7 worked after the Firefox
placement repair; that report does not establish full multi-monitor or performance coverage.

Download the Windows x64 ZIP, extract it to a new folder, quit the existing Deep
Lite using its tray menu, and run the new executable. Existing Deep Lite settings
are retained; a settings reset is not required. An NSIS installer is also available.
Build information, the source commit, CI link and SHA-256 sums accompany each release.
Executables are **unsigned** and require Microsoft Edge WebView2.

## Changes and settings

- Live blur remains the default for new settings. A 20 Hz frame budget, reduced
  resolution blur where compatible, cached blur images and three Ambient levels
  reduce work in the rendering code. WGC capture and final composition remain
  full resolution; no measured GPU reduction is claimed.
- Optional Solid, Stripes and Grid masks run without WGC/D3D blur workers. They
  obscure the background without real blur; DWM still composites the windows.
- Effect windows pass native mouse input through. Focus tracking, tool windows,
  hidden-owner grouping and retained focus after settings changes are repaired.
- When a background window refuses to move below the overlay, a guarded divider
  recovery preserves protected sharp windows and checks rollback on failure.
  This addresses the Firefox behavior observed during preview testing.

Existing explicit renderer choices and exclusions are preserved. To use blur,
select **Renderer → Live blur**. **Ignored apps stay sharp**: choose **Apply effect**
to remove an exclusion. “Keep … sharp (exclude from effects)” adds an exclusion;
it does not enable blur. There is no built-in Firefox or Tablacus exclusion.
Per-monitor/app-wide focus, tint, grain, desaturation, fades and configurable
hotkeys remain available.

## Known limits and validation

Always-on-top windows remain outside the effect. Fully occluded monitors are not
automatically suspended. The 48 Windows tests include synthetic native fixtures;
they do not run Firefox or establish complete three-monitor, display hot-plug,
resume, visual-quality or desktop-gesture coverage. The rebuilt stable binary
has not itself been exercised on the user's PC. Actual GPU savings remain unmeasured.

Use **Show focus diagnostics** for configuration and window-placement details.
New focus reports omit titles/URLs, but older log contents may still contain them;
review logs before sharing. See the [changelog](CHANGELOG.md),
[rendering limits and comparison procedure](docs/low-gpu-blur.md), and
[focus/input investigation](docs/focus-input-repair.md).

## Separate application identity

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
Windows code and runs 48 library tests, including native Win32 input, placement
and rollback fixtures. It builds/packages the exe and installer and records
source SHA, Authenticode status and hashes. It uses a standard GitHub-hosted Windows runner
in this public repository. Releases link the successful run for their exact SHA.

The upstream `release.yml` remains in the repository; `v*` tags trigger it.
Deep Lite stable releases use `deep-lite-v*` tags and verified artifacts from
the manual workflow instead. Previous previews remain available.

The installer’s optional app-data deletion uses the Tauri bundle-ID directories.
It does not remove `%APPDATA%\DeepLite`; these settings and logs remain after
uninstall and can be removed manually if no longer needed.
