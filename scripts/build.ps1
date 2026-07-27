# build.ps1 — Windows build script for L337 Audio Server
#
# Requires Rust/cargo installed. Run this on a Windows machine with the
# Rust toolchain. The finished binary is copied to .\bin for the installer
# to deploy.
#
# Usage:
#   .\scripts\build.ps1                        # native release build
#   .\scripts\build.ps1 -Target x86_64-pc-windows-gnu   # cross-build
[CmdletBinding()]
param(
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"

Write-Host "Building L337 Audio Server (release)..." -ForegroundColor Cyan

$env:CARGO_TARGET_DIR = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$env:TEMP\l337-build" }

if ($Target) {
    Write-Host "Building for target: $Target"
    cargo build --release --target $Target
    $src = Join-Path $env:CARGO_TARGET_DIR "$Target\release\l337-audio-server.exe"
} else {
    cargo build --release
    $src = Join-Path $env:CARGO_TARGET_DIR "release\l337-audio-server.exe"
}

if (-not (Test-Path $src)) {
    Write-Error "Build succeeded but binary not found at: $src"
    exit 1
}

$binDir = Join-Path $PSScriptRoot "..\bin"
if (-not (Test-Path $binDir)) {
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
}

Copy-Item -Path $src -Destination (Join-Path $binDir "l337-audio-server.exe") -Force
Write-Host "Build complete. Binary available at bin\l337-audio-server.exe" -ForegroundColor Green
