param([string]$Proxy = '', [string]$DataDirectory = '', [string]$DependencyDirectory = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (!$DataDirectory) { $DataDirectory = Join-Path $root '.build/托管依赖 网络验收' }
function EnvironmentSnapshot {
    $snapshot = [ordered]@{}
    foreach ($scope in @('User','Machine')) {
        $values = [Environment]::GetEnvironmentVariables($scope)
        $record = [ordered]@{}
        foreach ($key in $values.Keys | Sort-Object) { $record[$key] = $values[$key] }
        $snapshot[$scope] = $record
    }
    $snapshot | ConvertTo-Json -Depth 5 -Compress
}
$before = EnvironmentSnapshot
$originalPath = $env:PATH
$originalProxy = $env:SG_DEPENDENCY_TEST_PROXY
$originalDirectory = $env:SG_DEPENDENCY_TEST_DATA_DIR
$originalDependencies = $env:SG_DEPENDENCY_TEST_STORAGE
Push-Location $root
try {
    $env:SG_DEPENDENCY_TEST_PROXY = $Proxy
    $env:SG_DEPENDENCY_TEST_DATA_DIR = [IO.Path]::GetFullPath($DataDirectory)
    $env:SG_DEPENDENCY_TEST_STORAGE = if ($DependencyDirectory) { [IO.Path]::GetFullPath($DependencyDirectory) } else { '' }
    cargo test --release --locked --test dependencies official_download_and_actual_mcp -- --ignored --nocapture 2>&1 | Tee-Object 'artifacts/dependency-online-checks.txt'
    if ($LASTEXITCODE -ne 0) { throw 'Managed runtime/network acceptance failed' }
    $buildOutput = cargo test --release --locked --test dependencies --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency test build failed' }
    $executable = $buildOutput | ForEach-Object {
        try { $_ | ConvertFrom-Json } catch { }
    } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'dependencies' -and $_.executable } | Select-Object -Last 1 -ExpandProperty executable
    if (!$executable) { throw 'Missing dependency test executable' }
    # Environment applies only to this script and its child. Nothing writes the registry.
    $env:PATH = ''
    & $executable installed_offline_and_versions --ignored --nocapture 2>&1 | Tee-Object 'artifacts/dependency-offline-checks.txt'
    if ($LASTEXITCODE -ne 0) { throw 'Empty-PATH offline acceptance failed' }
    if ((EnvironmentSnapshot) -ne $before) { throw 'User/machine environment changed' }
    'PASS user and machine environment variables unchanged' | Add-Content 'artifacts/dependency-offline-checks.txt'
} finally {
    $env:PATH = $originalPath
    $env:SG_DEPENDENCY_TEST_PROXY = $originalProxy
    $env:SG_DEPENDENCY_TEST_DATA_DIR = $originalDirectory
    $env:SG_DEPENDENCY_TEST_STORAGE = $originalDependencies
    Pop-Location
}
