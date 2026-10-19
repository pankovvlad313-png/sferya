param([Parameter(Mandatory)][string]$Installer, [string]$KeyPath)
$ErrorActionPreference = 'Stop'
$sferyConfig = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'src-tauri\tauri.production.conf.json') | ConvertFrom-Json
$sferyReleaseInfo = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'release\release-info.json') | ConvertFrom-Json
if ($sferyReleaseInfo.version -ne $sferyConfig.version) { throw 'Release metadata version mismatch' }
$sferyInstaller = (Resolve-Path -LiteralPath $Installer).Path
$sferyTauri = Join-Path $PSScriptRoot 'node_modules\.bin\tauri.cmd'
$sferyArgs = @('signer', 'sign', '--app-version', $sferyConfig.version)
if ($KeyPath) { $sferyArgs += @('--private-key-path', (Resolve-Path -LiteralPath $KeyPath).Path) }
elseif (-not $env:TAURI_SIGNING_PRIVATE_KEY -and -not $env:TAURI_SIGNING_PRIVATE_KEY_PATH) { throw 'Set TAURI_SIGNING_PRIVATE_KEY or pass -KeyPath. Do not paste the key into chat.' }
$sferyArgs += $sferyInstaller
# Signing key and password are read by Tauri from the environment, never logged here.
& $sferyTauri @sferyArgs | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Updater signing failed' }
& node (Join-Path $PSScriptRoot 'scripts\verify-signature.cjs') $sferyInstaller
if ($LASTEXITCODE -ne 0) { throw 'Signing key does not match the key embedded in recovered Sfery. Do not publish.' }
$sferySignature = (Get-Content -Raw -LiteralPath ($sferyInstaller + '.sig')).Trim()
$sferyName = [IO.Path]::GetFileName($sferyInstaller)
$sferyManifest = [ordered]@{
    version = $sferyConfig.version
    notes = "$($sferyReleaseInfo.displayVersion). $($sferyReleaseInfo.notes)"
    pub_date = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = @{ 'windows-x86_64' = @{ url = "https://github.com/pankovvlad313-png/sferya/releases/download/v$($sferyConfig.version)/$sferyName"; signature = $sferySignature } }
}
$sferyManifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path (Split-Path -Parent $sferyInstaller) 'latest-v2.json') -Encoding utf8NoBOM
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'release\legacy-latest.json') -Destination (Join-Path (Split-Path -Parent $sferyInstaller) 'latest.json')
Write-Host 'Signature verified. New-channel latest-v2.json and unchanged legacy latest.json prepared locally; nothing published.'
