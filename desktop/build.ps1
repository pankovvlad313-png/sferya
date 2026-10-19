param([switch]$Release, [string]$TargetDir)
$ErrorActionPreference = 'Stop'
$sferyRoot = $PSScriptRoot
$sferyCargo = Get-Command cargo.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
if (-not $sferyCargo) {
    $sferyCargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $sferyCargo)) { throw 'Установите Rust MSVC и Microsoft C++ Build Tools с Windows SDK.' }
}
$sferyOldTarget = $env:CARGO_TARGET_DIR
$sferyOldJobs = $env:CARGO_BUILD_JOBS
try {
    $env:CARGO_BUILD_JOBS = '1'
    if ($TargetDir) { $env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath($TargetDir) }
    $sferyArguments = @('build', '--locked', '--manifest-path', (Join-Path $sferyRoot 'src-tauri\Cargo.toml'))
    if ($Release) { $sferyArguments += '--release' }
    & $sferyCargo @sferyArguments
    if ($LASTEXITCODE -ne 0) { throw "Сборка завершилась с кодом $LASTEXITCODE" }
    $sferyBuildRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $sferyRoot 'src-tauri\target' }
    $sferyProfile = if ($Release) { 'release' } else { 'debug' }
    $sferyOutputDir = Join-Path $sferyRoot 'dist'
    New-Item -ItemType Directory -Path $sferyOutputDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $sferyBuildRoot "$sferyProfile\sfery-recovered.exe") -Destination (Join-Path $sferyOutputDir 'Sfery-Recovered.exe')
    Write-Host "Готово: $(Join-Path $sferyOutputDir 'Sfery-Recovered.exe')"
} finally {
    if ($null -eq $sferyOldTarget) { Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue } else { $env:CARGO_TARGET_DIR = $sferyOldTarget }
    if ($null -eq $sferyOldJobs) { Remove-Item Env:CARGO_BUILD_JOBS -ErrorAction SilentlyContinue } else { $env:CARGO_BUILD_JOBS = $sferyOldJobs }
}
