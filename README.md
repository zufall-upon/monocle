# Monocle

A lightweight Windows overlay utility built with Tauri (Rust + WebView).

## Features

- Frameless, transparent settings window with acrylic blur
- Global shortcut support
- System tray integration
- Native Windows compositor effects (DWM / `SetWindowCompositionAttribute`)

## Stack

- **Backend:** Rust + Tauri 2 + `windows` crate
- **Frontend:** Vanilla HTML / CSS / JS
- **Target:** Windows 10/11

## Development

```sh
npm install
npm run dev
```

## Build

```sh
npm run build
```
