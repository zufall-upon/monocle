# Refused demotion: move the overlay divider

Diagnostic 6's supplied log shows two Firefox windows at the same integrity RID
(0x2000), UIAccess=0, as Deep Lite. Demotion returns success, but both windows
remain above root and every GPU HWND immediately and at the next observation.
Tablacus accepts demotion. This establishes a placement postcondition failure;
it does not identify the exact internal Firefox/Windows mechanism or implicate
the PortableApps launcher.

The repair keeps ordinary demotion and its bounded diagnostics. At reconciliation,
if a background still sits above the divider, move only our divider above it, then
re-pin the three GPU windows. The refusing application is not reconfigured.

Safeguards:
- Neither target nor root may be topmost.
- Every visible retained-sharp/ignored window must already be above the refusing
  target. Otherwise decline; do not sacrifice another monitor's retained app.
- Skip our owned visual windows when selecting the insertion point. Use HWND_TOP
  at the normal-band boundary; do not use HWND_TOPMOST.
- Check the resulting root and protected-window ordering. If invalid, attempt to
  restore the prior divider slot. One successful relocation per reconciliation.
- Never activate a window or change another application's style/owner. Existing
  input passthrough remains intact.

The existing native different-thread veto fixture calls this production helper
with root/tint/grain ownership and three GPU-style windows. It verifies a protected
anchor below the target prevents relocation, then verifies that the unprotected
refusing background falls below all GPU windows while the focused app stays above,
foreground stays unchanged, the root stays non-topmost, and visual windows do not
become the input target. The existing opaque DComp input fixture remains required.

This remains a preview: actual Firefox and three physical monitors are unverified.
A safe refusal is possible with incompatible retained/ignored ordering. Desktop
ordering observations are not atomic. Existing settings remain unchanged.

Short trial: activate, focus Firefox then Tablacus, wait 5 seconds, check both
Firefox windows. If still wrong, Show focus diagnostics includes bounded placement
samples and divider_fallback attempts/repairs/last result. No settings reset.
