$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting failed' }
    cargo clippy --all-targets --locked -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    cargo test --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo tests failed' }
} finally { Pop-Location }
