$ErrorActionPreference = 'Stop'
$sha = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sha -ne $env:GITHUB_SHA) { throw 'Build source SHA mismatch' }
git diff --exit-code
if ($LASTEXITCODE -ne 0) { throw 'Build modified tracked source files' }
$short = $sha.Substring(0, 12)
$destination = Join-Path $PWD 'dist'
$portable = Join-Path $destination 'portable'
New-Item -ItemType Directory -Force $portable | Out-Null
$exeName = "DeepLite-$short.exe"
Copy-Item 'src-tauri/target/release/deep-lite.exe' (Join-Path $destination $exeName)
Copy-Item 'src-tauri/target/release/deep-lite.exe' (Join-Path $portable $exeName)
$installers = @(Get-ChildItem 'src-tauri/target/release/bundle/nsis/*-setup.exe')
if ($installers.Count -ne 1) { throw 'Expected exactly one NSIS installer' }
Copy-Item $installers[0].FullName (Join-Path $destination "DeepLite-$short-x64-setup.exe")
Copy-Item LICENSE $portable
Copy-Item LICENSE $destination
# Inspect the generated installer without installing or launching it.
$nsiFiles = @(Get-ChildItem 'src-tauri/target/release' -Recurse -Filter installer.nsi)
if ($nsiFiles.Count -ne 1) { throw 'Expected one generated NSIS source for identity verification' }
$nsiText = Get-Content $nsiFiles[0].FullName -Raw
foreach ($definition in @('!define PRODUCTNAME "Deep Lite"', '!define MAINBINARYNAME "deep-lite"', '!define BUNDLEID "io.github.zufallupon.deeplite"')) {
    if (-not $nsiText.Contains($definition)) { throw "Installer identity mismatch: $definition" }
}
Copy-Item $nsiFiles[0].FullName (Join-Path $destination 'INSTALLER-SOURCE.txt')
$signatures = Get-ChildItem "$destination/*.exe" | ForEach-Object {
    $signature = Get-AuthenticodeSignature $_.FullName
    "$($_.Name): $($signature.Status)"
}
$info = @"
Deep Lite experimental Windows x64 build
Source commit: $sha
Source: https://github.com/$env:GITHUB_REPOSITORY/commit/$sha
CI: https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID

Compiled and packaged with the Tauri CLI on a standard GitHub-hosted Windows runner.
Rust policy tests passed in this run; GPU/UI runtime was NOT exercised.
PLACEMENT REPAIR PREVIEW: Firefox real-PC resolution is NOT confirmed.
Moves our divider above a refusing background only when protected windows stay above.
Bounded before/after/next-tick placement samples appear in Show focus diagnostics.
Independent shared-owner demotion repair included; no settings reset required.
Ignored/focus-retention follow-up: new settings default to Live blur; explicit prior
renderer choices (including Static) are preserved. Choose Live blur in settings.
Visual HWNDs are layered/nonactivating/input-transparent. First-click absorption
is removed; native app/desktop actions pass through. Tool-window classification
and hidden-owner grouping are repaired. Rebuilds now preserve real focus and refresh
Ignored caches from versioned snapshots. No saved exclusions are auto-removed.
Topmost windows remain outside the effect.
The native WindowFromPoint fixture does not establish real desktop gesture behavior.
Actual Firefox/Tablacus, 3-monitor operation and GPU savings remain unverified.
Operation counters: %TEMP%\deep-lite-gpu-blur.log (10-second cumulative samples).
Fully occluded monitors are not automatically suspended.
No code-signing certificate was supplied. Authenticode status:
$($signatures -join "`n")

Windows three-monitor behavior, visual quality, capture suspension/resume and
actual GPU savings remain unverified. This is an experimental prerelease.
The standalone exe requires the Microsoft Edge WebView2 runtime.

Deep Lite is separated from upstream Deep:
- settings/normal logs: %APPDATA%\DeepLite (no automatic import from %APPDATA%\Deep)
- GPU diagnostics: %TEMP%\deep-lite-gpu-blur.log
- autostart: HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Deep Lite
- installer/product: Deep Lite; identifier: io.github.zufallupon.deeplite
- installed executable: deep-lite.exe (distinct from upstream deep.exe)
Autostart defaults OFF. A default launch does not modify any Run value.
The upstream Run value named Deep is never written or deleted by this fork.
The standalone ZIP uses the same separate DeepLite settings as this installer;
it is not a fully portable per-folder settings profile.
Uninstall (including its app-data checkbox) retains %APPDATA%\DeepLite;
remove that fork-only settings/log folder manually if no longer needed.

The shared Local\DeepSingleInstance mutex intentionally prevents simultaneous
Deep / Deep Lite overlays. If either is already running, this app exits before
settings/log/autostart work; it does not stop the existing application. Use the
tray menu when you choose to switch apps. Hotkeys remain the upstream defaults.
No user-PC app was stopped, overwritten or run to produce this build.
Old unisolated build 4a6c844 is superseded and is not recommended for trial.

Upstream MIT copyright is retained in LICENSE. No additional billing settings
or paid runner configuration were used.
"@
$info | Set-Content (Join-Path $portable 'BUILD-INFO.txt') -Encoding utf8
$info | Set-Content (Join-Path $destination 'BUILD-INFO.txt') -Encoding utf8
Compress-Archive -Path "$portable/*" -DestinationPath (Join-Path $destination "DeepLite-$short-windows-x64.zip")
Get-ChildItem $destination -File | Sort-Object Name | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($_.Name)"
} | Set-Content (Join-Path $destination 'SHA256SUMS.txt') -Encoding ascii
Get-Content (Join-Path $destination 'SHA256SUMS.txt')
$info | Add-Content $env:GITHUB_STEP_SUMMARY
