# Changelog

## 0.1.1 — 2026-10-06

First stable Deep Lite release, based on user-confirmed Preview 7
(`5da199958507fe8cf0ddbbc6c9f0086e03f3c0c6`). Only version labels and release
documentation change from that preview; runtime Rust logic is unchanged.

- Retains real Live blur with a frame budget, compatible reduced-resolution blur,
  cached frames and simplified Ambient processing; adds optional capture-free masks.
- Repairs input pass-through, focus retention, hidden-owner grouping and exclusion
  refresh. Clarifies that ignored apps remain sharp.
- Adds guarded overlay-divider recovery for background windows that refuse
  demotion, including the reported Firefox case, with verified rollback handling.
- Keeps Deep Lite settings, logs, installer and autostart identity separate from Deep.

Validation uses 48 Windows tests plus executable/installer packaging and hash
checks. Preview 7 was reported working by the user; GPU savings and full
three-monitor/lifecycle coverage are not established. See [README](README.md)
for unsigned/WebView2 requirements, retained settings and update instructions.
