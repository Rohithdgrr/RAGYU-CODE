#!/usr/bin/env pwsh
$ErrorActionPreference = "Continue"
Write-Host "Starting build check..."
cargo check --color=always 2>&1 | Tee-Object -Variable output
$exitCode = $LASTEXITCODE
Write-Host "Exit code: $exitCode"
if ($exitCode -ne 0) {
    Write-Host "BUILD FAILED"
    Write-Host $output
} else {
    Write-Host "BUILD SUCCESS"
}
exit $exitCode
