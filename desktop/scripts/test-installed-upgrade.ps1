$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This test installs Sfery and is restricted to the disposable Actions runner.' }
$sferyRoot = Split-Path -Parent $PSScriptRoot
$sferyOutput = Join-Path $sferyRoot 'dist\update-1.45.0\verification'
$sferyInstall = Join-Path $env:RUNNER_TEMP 'SferyUpgradeInstall'
$sferyOriginal = Join-Path $sferyRoot '..\original\original-setup.exe'
$sferyCandidate = Join-Path $sferyRoot 'dist\update-1.45.0\Sfery_1.45.0_x64-setup.exe'
$sferyRegistry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sfery'
if (Test-Path -LiteralPath $sferyRegistry) { throw 'Runner already has Sfery; refusing to replace it.' }
New-Item -ItemType Directory -Path $sferyOutput -Force | Out-Null
& node (Join-Path $PSScriptRoot 'seed-fixture.cjs')
if ($LASTEXITCODE -ne 0) { throw 'Fixture setup failed' }
$sferySetup = Start-Process -FilePath $sferyOriginal -ArgumentList @('/S', "/D=$sferyInstall") -WindowStyle Hidden -PassThru -Wait
if ($sferySetup.ExitCode -ne 0) { throw 'Original installation failed' }
if ((Get-ItemProperty -LiteralPath $sferyRegistry).DisplayVersion -ne '1.44.0') { throw 'Original installer identity mismatch' }
$sferyExe = Join-Path $sferyInstall 'local-planner.exe'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9239'
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $env:RUNNER_TEMP 'SferyUpgradeWebView'
function Test-SferyStage([string]$Stage) {
    $sferyProcess = Start-Process -FilePath $sferyExe -WindowStyle Hidden -PassThru
    try {
        & node (Join-Path $PSScriptRoot 'installed-smoke.cjs') $Stage $sferyOutput
        if ($LASTEXITCODE -ne 0) { throw "$Stage application check failed" }
        if (-not $sferyProcess.WaitForExit(10000)) { throw "$Stage did not close cleanly" }
        if ($sferyProcess.ExitCode -ne 0) { throw "$Stage exited with $($sferyProcess.ExitCode)" }
    } finally {
        if (-not $sferyProcess.HasExited) { Stop-Process -Id $sferyProcess.Id -Force }
    }
}
Test-SferyStage 'original'
$sferySetup = Start-Process -FilePath $sferyCandidate -ArgumentList @('/S', '/UPDATE', "/D=$sferyInstall") -WindowStyle Hidden -PassThru -Wait
if ($sferySetup.ExitCode -ne 0) { throw 'Candidate installation failed' }
if ((Get-ItemProperty -LiteralPath $sferyRegistry).DisplayVersion -ne '1.45.0') { throw 'Candidate did not update the existing installation' }
Test-SferyStage 'candidate'
$sferyBackups = @(Get-ChildItem -LiteralPath (Join-Path $env:APPDATA 'local.sfery.planner\backups') -Directory -Filter 'pre-update-1.45.0-*' | Where-Object { $_.Name -notlike '*.partial' })
if ($sferyBackups.Count -ne 1) { throw 'Expected one verified pre-upgrade backup' }
$sferyManifest = Get-Content -Raw -LiteralPath (Join-Path $sferyBackups[0].FullName 'manifest.json') | ConvertFrom-Json
foreach ($sferyFile in $sferyManifest.files) {
    if ((Get-FileHash -LiteralPath (Join-Path $sferyBackups[0].FullName $sferyFile.path) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sferyFile.sha256) { throw 'Backup checksum mismatch' }
}
$sferyRollback = Join-Path $env:LOCALAPPDATA 'local.sfery.planner\rollback\1.44.0\local-planner.exe'
if ((Get-FileHash -LiteralPath $sferyRollback -Algorithm SHA256).Hash.ToLowerInvariant() -ne '7cd7820f7cae063982c30d3cbd9901152b8eee12173470ad830a322e43830dbd') { throw 'Old executable was not retained for rollback' }
Write-Host 'Installed 1.44.0 -> 1.45.0 upgrade verified on synthetic data; original user profile was not used.'
