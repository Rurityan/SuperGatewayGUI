param([switch]$Rebuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$commit = '292048a5a35c77984873bc741ffd196663c5a241'
$source = Join-Path $root '.build/upstream'
$engine = Join-Path $root 'engine'
$dependencyHash = (Get-FileHash "$PSScriptRoot/engine-package-lock.json" -Algorithm SHA256).Hash
New-Item -ItemType Directory -Force $engine | Out-Null
if (-not $Rebuild -and (Test-Path "$engine/manifest.json") -and (Test-Path "$engine/supergateway.exe")) {
    $manifest = Get-Content "$engine/manifest.json" -Raw | ConvertFrom-Json
    if ($manifest.commit -eq $commit -and $manifest.dependencyLockSha256 -eq $dependencyHash -and (Get-FileHash "$engine/supergateway.exe" -Algorithm SHA256).Hash -eq $manifest.sha256) { Write-Host 'Verified cached engine'; return }
}
if (-not (Test-Path "$source/.git")) {
    git clone https://github.com/supercorp-ai/supergateway.git $source
    if ($LASTEXITCODE -ne 0) { throw 'Cannot clone upstream' }
}
git -C $source fetch origin $commit
if ($LASTEXITCODE -ne 0) { throw 'Cannot fetch pinned upstream' }
git -C $source checkout --detach $commit
if ($LASTEXITCODE -ne 0) { throw 'Cannot checkout pinned upstream' }
if ((node --version) -notmatch '^v24\.') { throw 'Build requires Node 24 (users do not need Node)' }
Copy-Item "$PSScriptRoot/engine-package.json" "$source/package.json" -Force
Copy-Item "$PSScriptRoot/engine-package-lock.json" "$source/package-lock.json" -Force
Push-Location $source
try {
    npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    npm.cmd audit --omit=dev --audit-level=high
    if ($LASTEXITCODE -ne 0) { throw 'Runtime dependency audit failed' }
    npm.cmd run build
    if ($LASTEXITCODE -ne 0) { throw 'upstream build failed' }
    node scripts/build-binary.mjs
    if ($LASTEXITCODE -ne 0) { throw 'standalone build failed; Windows SDK signtool is required' }
} finally { Pop-Location }
Expand-Archive -LiteralPath "$source/.binary/supergateway-win-x64.zip" -DestinationPath $engine -Force
$help = & "$engine/supergateway.exe" --help 2>&1 | Out-String
foreach ($flag in @('--config','--checkConfig','--host','--apiKey','--logFormat','--streamableHttp')) { if (-not $help.Contains($flag)) { throw "Engine missing required option: $flag" } }
@{ version = (& "$engine/supergateway.exe" --version | Out-String).Trim(); commit = $commit; dependencyLockSha256 = $dependencyHash; node = (node --version); sha256 = (Get-FileHash "$engine/supergateway.exe" -Algorithm SHA256).Hash; source = 'https://github.com/supercorp-ai/supergateway'; builtUtc = [DateTime]::UtcNow.ToString('o') } | ConvertTo-Json | Set-Content "$engine/manifest.json" -Encoding utf8
Write-Host "Engine built and verified: $engine"
