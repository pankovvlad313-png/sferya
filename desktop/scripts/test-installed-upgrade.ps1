param([ValidateSet('1.44.0','1.45.0')][string]$FromVersion = '1.44.0')
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This installation test is restricted to the disposable Actions runner.' }
$sferyRoot = Split-Path -Parent $PSScriptRoot
$sferyVersion = (Get-Content -Raw -LiteralPath (Join-Path $sferyRoot 'src-tauri\tauri.production.conf.json') | ConvertFrom-Json).version
$sferyOutput = Join-Path $sferyRoot "dist\update-$sferyVersion\verification"
$sferyInstall = Join-Path $env:RUNNER_TEMP 'SferyUpgradeInstall'
$sferyOriginal = Join-Path $sferyRoot '..\original\original-setup.exe'
$sferyCandidate = Join-Path $sferyRoot "dist\update-$sferyVersion\Sfery_$($sferyVersion)_x64-setup.exe"
if ($FromVersion -eq '1.45.0') {
    $sferyOriginal = Join-Path $env:RUNNER_TEMP 'Sfery_1.45.0_x64-setup.exe'
    Invoke-WebRequest -Uri 'https://github.com/pankovvlad313-png/sferya/releases/download/v1.45.0/Sfery_1.45.0_x64-setup.exe' -OutFile $sferyOriginal
    if ((Get-FileHash -LiteralPath $sferyOriginal -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'd83302b34852df01c2168adb84e017af3d7caa889c598c22d5111fb799192f6e') { throw 'Published 1.45.0 installer identity mismatch' }
}
$sferyRegistry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sfery'
if (Test-Path -LiteralPath $sferyRegistry) { throw 'Runner already has Sfery; refusing to replace it.' }
New-Item -ItemType Directory -Path $sferyOutput -Force | Out-Null
& node (Join-Path $PSScriptRoot 'seed-fixture.cjs')
if ($LASTEXITCODE -ne 0) { throw 'Fixture setup failed' }
$sferySetup = Start-Process -FilePath $sferyOriginal -ArgumentList @('/S', "/D=$sferyInstall") -WindowStyle Hidden -PassThru -Wait
if ($sferySetup.ExitCode -ne 0) { throw 'Original installation failed' }
if ((Get-ItemProperty -LiteralPath $sferyRegistry).DisplayVersion -ne $FromVersion) { throw 'Original installer identity mismatch' }
$sferyOriginalHash = (Get-FileHash -LiteralPath (Join-Path $sferyInstall 'local-planner.exe') -Algorithm SHA256).Hash
& node (Join-Path $PSScriptRoot 'check-installed-data.cjs') before $sferyOutput
if ($LASTEXITCODE -ne 0) { throw 'Original baseline failed' }
$sferySetup = Start-Process -FilePath $sferyCandidate -ArgumentList @('/S', '/UPDATE', "/D=$sferyInstall") -WindowStyle Hidden -PassThru -Wait
if ($sferySetup.ExitCode -ne 0) { throw 'Candidate installation failed' }
$sferyRegistration = Get-ItemProperty -LiteralPath $sferyRegistry
if ($sferyRegistration.DisplayVersion -ne $sferyVersion) { throw 'Candidate did not update the existing installation' }
if ($sferyRegistration.InstallLocation.Trim('"') -ne $sferyInstall) { throw 'Installation path changed' }
if ((Get-FileHash -LiteralPath (Join-Path $sferyInstall 'local-planner.exe')).Hash -ne (Get-FileHash -LiteralPath (Join-Path $sferyRoot "dist\update-$sferyVersion\local-planner.exe")).Hash) { throw 'Installed executable differs from built executable' }
# Hosted Windows does not expose the original WebView debug endpoint. Exercise
# the exact shared storage initialization through a non-shipped Rust helper.
& cargo build --locked --release --no-default-features --features upgrade-probe --bin sfery-upgrade-probe --manifest-path (Join-Path $sferyRoot 'src-tauri\Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'Storage integration helper failed to build' }
$sferyProbe = Join-Path $sferyRoot 'src-tauri\target\release\sfery-upgrade-probe.exe'
$sferyData = Join-Path $env:APPDATA 'local.sfery.planner'
foreach ($sferyPass in @(1,2)) {
    $sferyReportPath = Join-Path $sferyOutput "storage-startup-$sferyPass.json"
    & $sferyProbe $sferyData $sferyReportPath
    if ($LASTEXITCODE -ne 0) { throw 'Shared storage startup failed' }
    $sferyReport = Get-Content -Raw -LiteralPath $sferyReportPath | ConvertFrom-Json
    if ($sferyReport.version -ne $sferyVersion -or $sferyReport.theme -ne 'dark' -or $sferyReport.counts.tasks -ne 1) { throw 'Original data was not loaded' }
}
& node (Join-Path $PSScriptRoot 'check-installed-data.cjs') after $sferyOutput
if ($LASTEXITCODE -ne 0) { throw 'Data preservation failed' }
$sferyBackups = @(Get-ChildItem -LiteralPath (Join-Path $sferyData 'backups') -Directory -Filter "pre-update-$sferyVersion-*" | Where-Object { $_.Name -notlike '*.partial' })
if ($sferyBackups.Count -ne 1) { throw 'Expected one verified pre-upgrade backup' }
$sferyManifest = Get-Content -Raw -LiteralPath (Join-Path $sferyBackups[0].FullName 'manifest.json') | ConvertFrom-Json
foreach ($sferyFile in $sferyManifest.files) {
    if ((Get-FileHash -LiteralPath (Join-Path $sferyBackups[0].FullName $sferyFile.path) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sferyFile.sha256) { throw 'Backup checksum mismatch' }
}
$sferyRollback = Join-Path $env:LOCALAPPDATA "local.sfery.planner\rollback\$FromVersion\local-planner.exe"
if ((Get-FileHash -LiteralPath $sferyRollback -Algorithm SHA256).Hash -ne $sferyOriginalHash) { throw 'Old executable was not retained for rollback' }
Write-Host 'Installed upgrade, shared storage startup, row preservation, attachments and verified backup passed.'
