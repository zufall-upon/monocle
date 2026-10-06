> **Experimental low-GPU fork:** Background blur updates are budgeted to 20/s,
> with capture suspension while disabled and cached-frame reuse. Strong Deep
> blur uses half-size effect bitmaps; weak Deep blur and Ambient retain the native
> path. Windows runtime behavior and GPU savings have not yet been measured. See [implementation and validation status](docs/low-gpu-blur.md).
> The Install links below are upstream releases, not binaries from this fork.

<div align="center">

# Deep

**A lightweight Windows focus overlay that blurs everything except what you're working on.**

When Deep is active, your whole screen is gently blurred, tinted, and grained —
except the window you're focused on, which stays crisp. It's a calm, distraction-free
"spotlight" for whatever has your attention. Lives in the system tray, toggles with a
hotkey or a quick shake of the mouse.

Built with [Tauri 2](https://tauri.app) (Rust + WebView). Windows 10/11, x64.

<!-- TODO: drop a screenshot or GIF of the settings window + the overlay in action here, e.g.
     ![Deep](docs/screenshot.png) -->

</div>

---

## Features

- **Focus spotlight** — blurs the entire screen except your focused window.
- **Two blur modes** — *Deep focus* (uniform full-screen blur) and *Ambient*
  (progressive: sharp at the top of the screen, ramping to full blur at the bottom).
- **Tint & grain** — color-tint the blur (10 presets + custom picker) and add a
  film-grain texture, each with its own intensity.
- **Mono** — optionally desaturate the blurred area to grayscale.
- **Shake to toggle** — give your mouse a shake to flip Deep on or off
  (sensitivity adjustable).
- **Per-monitor focus** — only blur the monitors you're *not* working on.
- **App-wide focus** — keep *every* window of the focused app sharp, not just the active one.
- **Ignored apps** — pin chosen apps to always stay sharp, regardless of focus.
- **Auto-hide taskbar** and **hide desktop icons** while active (both restored on deactivate).
- **Smooth crossfades** — tunable fade-in/out duration.
- **Global hotkeys** for toggling, switching modes, and opening settings.
- **Start on login** — launch automatically with Windows (per-user, no admin needed).
- **System tray** integration.

## Install

1. Go to the [**Releases**](https://github.com/brycelewiswork/deep/releases) page.
2. Download the latest **`Deep_x.y.z_x64-setup.exe`** (or the `.msi` if you prefer).
3. Run it and follow the installer.

> **Heads up — unsigned build.** Deep isn't code-signed, so Windows SmartScreen
> may show *"Windows protected your PC."* Click **More info → Run anyway** to continue.
> (Building your own from source avoids this — see below.)

Once installed, Deep starts in the system tray. Open settings with the tray icon or
`Ctrl + Alt + Win + C`, then **Activate** (or shake your mouse, or press
`Ctrl + Alt + Win + F`).

## Default shortcuts

| Action | Shortcut |
| --- | --- |
| Toggle Deep on/off | `Ctrl + Alt + Win + F` |
| Switch blur mode | `Ctrl + Alt + Win + M` |
| Show/hide settings window | `Ctrl + Alt + Win + C` |

All three are rebindable in the settings window.

## Customizing it

Deep stores its configuration as plain JSON at:

```
%APPDATA%\Deep\settings.json
```

Everything in the settings window is written here — blur intensity and mode, tint
color/opacity, grain, shake sensitivity, fade duration, focus options, ignored apps,
hotkeys, and start-on-login. You can edit it by hand or just use the UI.

To change behavior beyond what the settings expose, fork the repo and build from source —
the Rust backend lives in [`src-tauri/src`](src-tauri/src) and the settings UI is plain
HTML/CSS/JS in [`src/`](src).

## Build from source

**Prerequisites**

- [Node.js](https://nodejs.org) 18+
- [Rust](https://rustup.rs) (stable)
- **Microsoft C++ Build Tools** (the "Desktop development with C++" workload) — Tauri
  needs the MSVC toolchain to compile on Windows. See the
  [Tauri prerequisites](https://tauri.app/start/prerequisites/) for details.

**Run in development**

```sh
npm install
npm run dev
```

`npm run dev` compiles the Rust backend and launches the app with live-reload of the
frontend — no separate dev server needed.

**Build an installer**

```sh
npm run build
```

The release binary embeds the frontend, and the installers are written to:

```
src-tauri/target/release/bundle/nsis/   # Deep_x.y.z_x64-setup.exe
src-tauri/target/release/bundle/msi/    # Deep_x.y.z_x64_en-US.msi
```

## Releases / CI

Tagged releases are built automatically. Pushing a `v*` tag (e.g. `v0.1.0`) triggers a
[GitHub Actions workflow](.github/workflows/release.yml) that builds the Windows
installers on a `windows-latest` runner and publishes them to a GitHub Release.

## Tech

- **Backend:** Rust + Tauri 2 + the [`windows`](https://crates.io/crates/windows) crate
- **Compositing:** Windows.Graphics.Capture + Direct3D/Direct2D GPU blur, DirectComposition
- **Frontend:** vanilla HTML / CSS / JS
- **Target:** Windows 10/11 (x64)

## Contributing

Issues and pull requests are welcome. Fork it, make it yours, and send improvements back
if you'd like. For larger changes, opening an issue first to discuss is appreciated.

## License

[MIT](LICENSE) © 2026 Bryce Lewis
