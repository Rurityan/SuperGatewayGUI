$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $root ('.build/source-package-' + [guid]::NewGuid().ToString('N'))
$source = Join-Path $stage 'SuperGatewayGUI-source'
New-Item -ItemType Directory -Force "$source/.cargo", "$source/legacy/dotnet" | Out-Null
foreach ($name in @('Cargo.toml','Cargo.lock','build.rs','app.manifest','README.md', 'README.en.md','DEVELOPMENT.md','CHANGELOG.md','VALIDATION.md','DEPENDENCIES_VALIDATION.md','STORAGE_VALIDATION.md','ONESHOT_VALIDATION.md','ENVIRONMENT_VALIDATION.md','THIRD_PARTY_NOTICES.md','LICENSE','.gitignore')) {
    Copy-Item -LiteralPath "$root/$name" $source
}
Copy-Item "$root/.cargo/config.toml" "$source/.cargo"
foreach ($name in @('src','scripts','tests','assets')) { Copy-Item -LiteralPath "$root/$name" $source -Recurse }
New-Item -ItemType Directory -Force "$source/verification" | Out-Null
foreach ($file in Get-ChildItem "$root/artifacts" -File -Filter 'oneshot-*.txt') { Copy-Item -LiteralPath $file.FullName "$source/verification" }
foreach ($file in Get-ChildItem "$root/artifacts" -File -Filter 'package-save-*.txt') { Copy-Item -LiteralPath $file.FullName "$source/verification" }
foreach ($file in Get-ChildItem "$root/artifacts" -File -Filter 'environment-*.txt') { Copy-Item -LiteralPath $file.FullName "$source/verification" }
foreach ($name in @('tls-checks.txt','tls-protocol-checks.txt','tls-ui-checks.txt','tls-measurements.json','tls-process-roles.txt','dependency-regression-checks.txt','dependency-final-unit-checks.txt','dependency-online-checks.txt','dependency-offline-checks.txt','dependency-ui-checks.txt','dependency-installed-ui-checks.txt','dependency-packaged-ui-checks.txt','dependency-disk-usage.json','storage-unit-checks.txt','storage-ui-checks.txt','storage-regression-checks.txt','storage-packaged-ui-checks.txt','storage-network-checks.txt')) {
    if (Test-Path "$root/artifacts/$name") { Copy-Item -LiteralPath "$root/artifacts/$name" "$source/verification" }
}
foreach ($file in $(if (Test-Path "$root/legacy/dotnet") { Get-ChildItem "$root/legacy/dotnet" -File -Recurse }) | Where-Object {
    $_.FullName -notmatch '[\\/](bin|obj|artifacts)[\\/]' -and $_.Extension -in '.cs','.csproj','.xaml','.json','.md','.ps1'
}) {
    $relative = [IO.Path]::GetRelativePath("$root/legacy/dotnet", $file.FullName)
    $destination = Join-Path "$source/legacy/dotnet" $relative
    New-Item -ItemType Directory -Force (Split-Path $destination) | Out-Null
    Copy-Item -LiteralPath $file.FullName $destination
}
$zip = "$root/artifacts/SuperGatewayGUI-source.zip"
Compress-Archive -Path $source -DestinationPath $zip -Force
"$((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower())  SuperGatewayGUI-source.zip" | Set-Content "$zip.sha256" -Encoding ascii
Write-Host "Packaged $zip"
