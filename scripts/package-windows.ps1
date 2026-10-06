$ErrorActionPreference = 'Stop'
$sha = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sha -ne $env:GITHUB_SHA) { throw 'Build source SHA mismatch' }
git diff --exit-code
if ($LASTEXITCODE -ne 0) { throw 'Build modified tracked source files' }
$short = $sha.Substring(0, 12)
$destination = Join-Path $PWD 'dist'
$portable = Join-Path $destination 'portable'
New-Item -ItemType Directory -Force $portable | Out-Null
$exeName = "Monocle-low-gpu-$short.exe"
Copy-Item 'src-tauri/target/release/deep.exe' (Join-Path $destination $exeName)
Copy-Item 'src-tauri/target/release/deep.exe' (Join-Path $portable $exeName)
$installers = @(Get-ChildItem 'src-tauri/target/release/bundle/nsis/*-setup.exe')
if ($installers.Count -ne 1) { throw 'Expected exactly one NSIS installer' }
Copy-Item $installers[0].FullName (Join-Path $destination "Monocle-low-gpu-$short-x64-setup.exe")
Copy-Item LICENSE $portable
Copy-Item LICENSE $destination
$signatures = Get-ChildItem "$destination/*.exe" | ForEach-Object {
    $signature = Get-AuthenticodeSignature $_.FullName
    "$($_.Name): $($signature.Status)"
}
$info = @"
Monocle / Deep low-GPU experimental Windows x64 build
Source commit: $sha
Source: https://github.com/$env:GITHUB_REPOSITORY/commit/$sha
CI: https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID

Compiled and packaged with the Tauri CLI on a standard GitHub-hosted Windows runner.
Rust policy tests passed in this run; GPU/UI runtime was NOT exercised.
No code-signing certificate was supplied. Authenticode status:
$($signatures -join "`n")

Windows three-monitor behavior, visual quality, capture suspension/resume and
actual GPU savings remain unverified. This is an experimental prerelease.
The standalone exe requires the Microsoft Edge WebView2 runtime.

This fork retains upstream Deep's application identity, settings path and hotkeys.
Do not install over or launch alongside an existing Deep installation without
first deciding how to preserve that installation and its settings. The installer
can replace the upstream app. Extracting this ZIP does not make settings isolated.
No user-PC app was stopped, overwritten or run to produce this build.

Upstream MIT copyright is retained in LICENSE. No additional billing settings
or paid runner configuration were used.
"@
$info | Set-Content (Join-Path $portable 'BUILD-INFO.txt') -Encoding utf8
$info | Set-Content (Join-Path $destination 'BUILD-INFO.txt') -Encoding utf8
Compress-Archive -Path "$portable/*" -DestinationPath (Join-Path $destination "Monocle-low-gpu-$short-windows-x64.zip")
Get-ChildItem $destination -File | Sort-Object Name | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($_.Name)"
} | Set-Content (Join-Path $destination 'SHA256SUMS.txt') -Encoding ascii
Get-Content (Join-Path $destination 'SHA256SUMS.txt')
$info | Add-Content $env:GITHUB_STEP_SUMMARY
