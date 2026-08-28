#!/usr/bin/env pwsh
# Test script to verify shell injection fix

Write-Host "Building project..." -ForegroundColor Cyan
cargo build --manifest-path "$PSScriptRoot\Cargo.toml" 2>&1 | Out-Host

if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

Write-Host "`nRunning shell injection prevention tests..." -ForegroundColor Cyan
cargo test --manifest-path "$PSScriptRoot\Cargo.toml" --lib parse_command_to_argv 2>&1 | Out-Host

if ($LASTEXITCODE -eq 0) {
    Write-Host "`nTests passed!" -ForegroundColor Green
} else {
    Write-Host "`nTests failed!" -ForegroundColor Red
    exit 1
}
