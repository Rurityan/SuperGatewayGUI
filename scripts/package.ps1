param([switch]$RebuildEngine)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    & "$PSScriptRoot/build-engine.ps1" -Rebuild:$RebuildEngine
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
    $publish = Join-Path $root 'artifacts/SuperGatewayGUI-win-x64'
    if (Test-Path "$publish/SuperGatewayGUI.exe") {
        $old = Join-Path $root 'legacy/dotnet/artifacts/SuperGatewayGUI-win-x64'
        $currentHash = (Get-FileHash "$publish/SuperGatewayGUI.exe").Hash
        if ((Get-Item "$publish/SuperGatewayGUI.exe").Length -gt 50MB) {
            if (!(Test-Path "$old/SuperGatewayGUI.exe") -or (Get-FileHash "$old/SuperGatewayGUI.exe").Hash -ne $currentHash) { throw 'Old baseline must be archived before replacing it' }
        }
    }
    # Recreate only the explicitly resolved publish directory inside this workspace.
    if (Test-Path $publish) {
        $resolved = (Resolve-Path $publish).Path
        if ($resolved -ne "$root\artifacts\SuperGatewayGUI-win-x64") { throw 'Unexpected publish path' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    New-Item -ItemType Directory -Force "$publish/engine", "$publish/licenses" | Out-Null
    Copy-Item 'target/x86_64-pc-windows-msvc/release/SuperGatewayGUI.exe' $publish
    Copy-Item 'engine/supergateway.exe', 'engine/manifest.json', 'engine/LICENSE', 'engine/THIRD_PARTY_LICENSES.txt' "$publish/engine"
    Copy-Item 'README.md', 'DEVELOPMENT.md', 'CHANGELOG.md', 'VALIDATION.md', 'DEPENDENCIES_VALIDATION.md', 'STORAGE_VALIDATION.md', 'ONESHOT_VALIDATION.md', 'ENVIRONMENT_VALIDATION.md', 'THIRD_PARTY_NOTICES.md', 'LICENSE' $publish
    $metadata = cargo metadata --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $licenses = @()
    foreach ($package in $metadata.packages | Where-Object source) {
        $directory = Split-Path $package.manifest_path -Parent
        $destination = "$publish/licenses/$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Force $destination | Out-Null
        $files = Get-ChildItem -LiteralPath $directory -File | Where-Object Name -Match '^(LICENSE|LICENCE|COPYING|NOTICE)'
        foreach ($file in $files) { Copy-Item -LiteralPath $file.FullName $destination }
        $licenses += "$($package.name) $($package.version): $($package.license)"
    }
    $licenses | Set-Content "$publish/licenses/RUST-DEPENDENCIES.txt" -Encoding utf8
    $zip = "$root/artifacts/SuperGatewayGUI-win-x64.zip"
    Compress-Archive -Path $publish -DestinationPath $zip -Force
    "$((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower())  SuperGatewayGUI-win-x64.zip" | Set-Content "$zip.sha256" -Encoding ascii
    Write-Host "Packaged $zip"
    & "$PSScriptRoot/package-source.ps1"
} finally { Pop-Location }
