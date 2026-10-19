param([string]$Destination)
$ErrorActionPreference = 'Stop'
$sferyRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) { $Destination = Join-Path $sferyRoot '..\original' }
$sferyHash = '7cd7820f7cae063982c30d3cbd9901152b8eee12173470ad830a322e43830dbd'
$sferyOriginal = Join-Path $Destination 'local-planner.exe'
if (Test-Path -LiteralPath $sferyOriginal) {
    if ((Get-FileHash -LiteralPath $sferyOriginal -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sferyHash) { throw 'Original EXE hash mismatch' }
    return
}
$sferyStage = Join-Path ([IO.Path]::GetTempPath()) ('sfery-original-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sferyStage | Out-Null
$sferySetup = Join-Path $sferyStage 'original-setup.exe'
Invoke-WebRequest -Uri 'https://github.com/pankovvlad313-png/sferya/releases/download/v1.44.0/Sfery_1.44.0_x64-setup.exe' -OutFile $sferySetup
Invoke-WebRequest -Uri 'https://github.com/pankovvlad313-png/sferya/releases/download/v1.44.0/Sfery_1.44.0_x64-setup.exe.sig' -OutFile ($sferySetup + '.sig')
& node (Join-Path $PSScriptRoot 'verify-signature.cjs') $sferySetup --legacy
if ($LASTEXITCODE -ne 0) { throw 'Original installer signature invalid' }
$sfery7z = Get-Command 7z.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
if (-not $sfery7z) { $sfery7z = 'C:\Program Files\7-Zip\7z.exe' }
# Extract only: never execute the old installer on the build runner.
& $sfery7z x $sferySetup "-o$sferyStage\unpacked" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not extract original installer' }
$sferyFound = @(Get-ChildItem -LiteralPath (Join-Path $sferyStage 'unpacked') -Filter local-planner.exe -Recurse)
if ($sferyFound.Count -ne 1 -or (Get-FileHash -LiteralPath $sferyFound[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sferyHash) { throw 'Original EXE hash mismatch' }
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
Copy-Item -LiteralPath $sferyFound[0].FullName -Destination $sferyOriginal
Copy-Item -LiteralPath $sferySetup -Destination (Join-Path $Destination 'original-setup.exe')
Write-Host 'Original v1.44.0 executable verified; installer was not executed.'
