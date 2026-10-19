param([string]$TargetDir)
$ErrorActionPreference = 'Stop'
$sferyRoot = $PSScriptRoot
$sferyVersion = (Get-Content -Raw -LiteralPath (Join-Path $sferyRoot 'src-tauri\tauri.production.conf.json') | ConvertFrom-Json).version
$sferyOldPath = $env:PATH
$sferyOldTarget = $env:CARGO_TARGET_DIR
$sferyOldJobs = $env:CARGO_BUILD_JOBS
Push-Location $sferyRoot
try {
    $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
    if ($TargetDir) { $env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath($TargetDir) }
    $env:CARGO_BUILD_JOBS = '2'
    & npm.cmd ci --ignore-scripts --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    & node --test scripts\verify-signature.test.cjs
    if ($LASTEXITCODE -ne 0) { throw 'Signature checks failed' }
    & (Join-Path $sferyRoot 'scripts\get-original.ps1')
    & cargo test --locked --manifest-path src-tauri\Cargo.toml --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed' }
    & (Join-Path $sferyRoot 'node_modules\.bin\tauri.cmd') build --config src-tauri/tauri.production.conf.json --features production-upgrade --ci -- --locked --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'Update build failed' }
    $sferyBuild = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $sferyRoot 'src-tauri\target' }
    $sferySetup = @(Get-ChildItem -LiteralPath (Join-Path $sferyBuild 'release\bundle\nsis') -Filter "*$sferyVersion*x64*setup.exe")
    if ($sferySetup.Count -ne 1) { throw "Expected exactly one v$sferyVersion x64 installer" }
    $sferyOut = Join-Path $sferyRoot "dist\update-$sferyVersion"
    New-Item -ItemType Directory -Path $sferyOut -Force | Out-Null
    Copy-Item -LiteralPath $sferySetup[0].FullName -Destination $sferyOut
    # Validate and expose the exact binary shipped inside NSIS, including its bundle stamp.
    $sferyPayloadDir = Join-Path ([IO.Path]::GetTempPath()) ('sfery-payload-' + [Guid]::NewGuid().ToString('N'))
    $sfery7z = Get-Command 7z.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
    if (-not $sfery7z) { $sfery7z = 'C:\Program Files\7-Zip\7z.exe' }
    & $sfery7z x $sferySetup[0].FullName "-o$sferyPayloadDir" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect installer payload' }
    $sferyPayload = @(Get-ChildItem -LiteralPath $sferyPayloadDir -Filter local-planner.exe -Recurse)
    if ($sferyPayload.Count -ne 1) { throw 'Expected exactly one application executable inside the installer' }
    if ($sferyPayload[0].VersionInfo.ProductVersion -ne $sferyVersion) { throw 'Installer contains the wrong application version' }
    Copy-Item -LiteralPath $sferyPayload[0].FullName -Destination (Join-Path $sferyOut 'local-planner.exe')
    [ordered]@{compilerExe=(Get-FileHash -LiteralPath (Join-Path $sferyBuild 'release\local-planner.exe')).Hash;installerPayload=(Get-FileHash -LiteralPath $sferyPayload[0].FullName).Hash;version=$sferyPayload[0].VersionInfo.ProductVersion} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $sferyOut 'payload-verification.json') -Encoding utf8NoBOM
    Copy-Item -LiteralPath (Join-Path $sferyRoot 'release\legacy-latest.json') -Destination (Join-Path $sferyOut 'latest.json')
    Copy-Item -LiteralPath (Join-Path $sferyRoot 'release\Update-Sfery.cmd') -Destination $sferyOut
    Copy-Item -LiteralPath (Join-Path $sferyRoot 'release\MANUAL-UPGRADE.md') -Destination $sferyOut
    Get-ChildItem -LiteralPath $sferyOut -File -Filter '*.exe' | Get-FileHash -Algorithm SHA256 | Select-Object @{N='file';E={Split-Path $_.Path -Leaf}},Hash | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $sferyOut 'checksums.json') -Encoding utf8NoBOM
    Write-Host "Unsigned candidate ready: $sferyOut. Test it before signing or publishing."
} finally {
    Pop-Location
    $env:PATH = $sferyOldPath
    if ($null -eq $sferyOldTarget) { Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue } else { $env:CARGO_TARGET_DIR = $sferyOldTarget }
    if ($null -eq $sferyOldJobs) { Remove-Item Env:CARGO_BUILD_JOBS -ErrorAction SilentlyContinue } else { $env:CARGO_BUILD_JOBS = $sferyOldJobs }
}
