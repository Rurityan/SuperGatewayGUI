param([string]$Gui = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
function EnvironmentSnapshot {
    $snapshot = [ordered]@{}
    foreach ($scope in @('User', 'Machine')) {
        $values = [Environment]::GetEnvironmentVariables($scope)
        $record = [ordered]@{}
        foreach ($key in $values.Keys | Sort-Object) { $record[$key] = $values[$key] }
        $snapshot[$scope] = $record
    }
    $snapshot | ConvertTo-Json -Depth 5 -Compress
}
$before = EnvironmentSnapshot
$original = $env:SG_ENVIRONMENT_GUI
Push-Location $root
try {
    $env:SG_ENVIRONMENT_GUI = if ($Gui) { [IO.Path]::GetFullPath($Gui) } else { '' }
    cargo test --release --locked --test environment --test oneshot -- --nocapture 2>&1 | Tee-Object artifacts/environment-policy-checks.txt
    if ($LASTEXITCODE -ne 0) { throw 'Service environment policy tests failed' }
    if ((EnvironmentSnapshot) -ne $before) { throw 'User or machine environment changed' }
    'PASS Windows user and machine environment variables unchanged' | Add-Content artifacts/environment-policy-checks.txt
} finally {
    $env:SG_ENVIRONMENT_GUI = $original
    Pop-Location
}
