# Build the PFM workspace (release). Windows PowerShell script.
$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

Write-Host "==> cargo fmt --check"
cargo fmt --check

Write-Host "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

Write-Host "==> cargo build --release"
cargo build --release

Write-Host "Build complete: target\release\pfm-app.exe"