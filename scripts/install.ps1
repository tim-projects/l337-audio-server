# install.ps1 — Windows installer for L337 Audio Server
#
# Installs the server as a Windows service.
# This is a placeholder implementation.
[CmdletBinding()]
param(
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$ErrorActionPreference = "Stop"

Write-Host "`n[INFO] Windows installer — not yet implemented" -ForegroundColor Cyan
Write-Host "`nThis is a placeholder. Please install manually or run the server directly:" -ForegroundColor Yellow
Write-Host "  .\bin\l337-audio-server.exe`n" -ForegroundColor Yellow

# TODO: Create a Windows Service using New-Service or sc.exe
# Example:
#   New-Service -Name "l337-audio-server" -BinaryPathName "C:\Program Files\l337-audio-server\l337-audio-server.exe" -DisplayName "L337 Audio Server"

throw "Windows installer not yet implemented"
