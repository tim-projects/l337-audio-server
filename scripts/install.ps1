# install.ps1 — Windows installer for L337 Audio Server
#
# Downloads prebuilt binaries from GitHub releases and installs them
# as a Windows service for all users. Registers itself in Add/Remove
# Programs and supports UAC self-elevation.
#
# Requirements:
#   - PowerShell 5.1 or later
#   - Internet access to GitHub Releases
#
# Usage:
#   .\install.ps1                           # install or update latest stable release
#   .\install.ps1 -PreRelease               # install or update latest prerelease
#   .\install.ps1 -Uninstall                 # remove installation
#   .\install.ps1 -Uninstall -RemoveData      # remove installation and data
#   .\install.ps1 -DryRun                     # show what would happen
#   .\install.ps1 -Force                       # force reinstall even if same version
[CmdletBinding()]
param(
    [switch]$DryRun,
    [switch]$PreRelease,
    [switch]$Force,
    [switch]$Uninstall,
    [switch]$RemoveData
)

$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------------------
# Self-elevate to Administrator if needed
# ---------------------------------------------------------------------------
function Ensure-Administrator {
    $currentPrincipal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Host "[INFO] Requesting administrator privileges..." -ForegroundColor Cyan
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = "powershell.exe"
        $psi.Arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`""
        $psi.Verb = "runas"
        $psi.UseShellExecute = $true
        try {
            [System.Diagnostics.Process]::Start($psi) | Out-Null
        } catch {
            Write-Fail "Failed to elevate privileges: $_"
        }
        exit 0
    }
}

Ensure-Administrator

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
$Repo = "tim-projects/l337-audio-server"
$InstallDir = "C:\Program Files\l337-audio-server"
$DataDir    = "C:\ProgramData\l337-audio-server"
$ServiceName = "l337-audio-server"
$ServiceDisplayName = "L337 Audio Server"
$WrapperName = "start-l337.cmd"
$ArpPath = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ServiceName"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
function Write-Info($msg) {
    Write-Host "[INFO] $msg" -ForegroundColor Cyan
}
function Write-Ok($msg) {
    Write-Host "[OK]   $msg" -ForegroundColor Green
}
function Write-Warn($msg) {
    Write-Host "[WARN] $msg" -ForegroundColor Yellow
}
function Write-Fail($msg) {
    Write-Host "[FAIL] $msg" -ForegroundColor Red
    exit 1
}

function Get-GithubRelease {
    param([string]$Endpoint)
    $url = "https://api.github.com/repos/$Repo/$Endpoint"
    try {
        $response = Invoke-RestMethod -Uri $url -Method Get -ErrorAction Stop
        return $response
    } catch {
        Write-Fail "Failed to contact GitHub Releases API: $_"
    }
}

function Get-AssetName {
    $arch = "x86_64"
    if ([Environment]::Is64BitOperatingSystem) {
        $procArch = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
        $procArchW6432 = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITEW6432")
        if ($procArch -eq "ARM64" -or $procArchW6432 -eq "ARM64") {
            $arch = "aarch64"
        }
    }
    "l337-audio-server-${arch}-windows-msvc.exe"
}

function Download-Binary {
    param([string]$Url, [string]$Dest)
    $tmp = "$Dest.tmp.$$"
    Write-Info "Downloading: $Url"
    try {
        Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing -ErrorAction Stop
    } catch {
        Remove-Item -Path $tmp -Force -ErrorAction SilentlyContinue
        Write-Fail "Failed to download binary from $Url : $_"
    }
    if (-not (Test-Path $tmp) -or (Get-Item $tmp).Length -eq 0) {
        Remove-Item -Path $tmp -Force -ErrorAction SilentlyContinue
        Write-Fail "Downloaded file is empty. Check the release URL."
    }
    Move-Item -Path $tmp -Destination $Dest -Force
    $size = (Get-Item $Dest).Length / 1KB
    Write-Ok "Downloaded: $Dest ($([math]::Round($size, 1)) KB)"
}

function New-ServiceWrapper {
    param([string]$InstallDir, [string]$Token)
    $wrapperPath = Join-Path $InstallDir $WrapperName
    $exePath = Join-Path $InstallDir "l337-audio-server.exe"
    $logPath = Join-Path $DataDir "$ServiceName.log"
    $content = @"
@echo off
set L337__SERVER__HOST=0.0.0.0
set L337__SERVER__PORT=1337
set L337__SERVER__TOKEN=$Token
set L337__SERVER__DUMMY=false
set L337__SERVER__TRANSPORT=auto
cd /d "%~dp0"
"$exePath" >> "$logPath" 2>&1
"@
    $content | Out-File -FilePath $wrapperPath -Encoding ASCII
    Write-Ok "Created service wrapper: $wrapperPath"
    return $wrapperPath
}

function Install-Service {
    param([string]$WrapperPath)
    Write-Info "Configuring Windows service: $ServiceName"
    $binPathArg = "`"$WrapperPath`""
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        Write-Info "Stopping existing service..."
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        sc.exe delete $ServiceName | Out-Null
    }
    sc.exe create $ServiceName binPath= $binPathArg start= auto DisplayName= $ServiceDisplayName | Out-Null
    sc.exe description $ServiceName "L337 Audio Server — lightweight multi-room audio streaming server." | Out-Null
    sc.exe start $ServiceName | Out-Null
    Write-Ok "Windows service installed and started"
}

function Uninstall-Service {
    Write-Info "Uninstalling Windows service..."
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        sc.exe delete $ServiceName | Out-Null
    }
    Write-Ok "Service removed"
}

function New-DefaultToken {
    -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 32 | ForEach-Object {[char]$_})
}

function New-ArpEntry {
    param([string]$Version)
    Write-Info "Registering in Add/Remove Programs..."
    if (Test-Path $ArpPath) {
        Remove-Item -Path $ArpPath -Force | Out-Null
    }
    New-Item -Path $ArpPath -Force | Out-Null
    Set-ItemProperty -Path $ArpPath -Name "DisplayName" -Value $ServiceDisplayName
    Set-ItemProperty -Path $ArpPath -Name "DisplayVersion" -Value $Version
    Set-ItemProperty -Path $ArpPath -Name "Publisher" -Value "L337 Audio Server"
    Set-ItemProperty -Path $ArpPath -Name "InstallLocation" -Value $InstallDir
    Set-ItemProperty -Path $ArpPath -Name "UninstallString" -Value "`"$PSCommandPath`" -Uninstall"
    Set-ItemProperty -Path $ArpPath -Name "DisplayIcon" -Value (Join-Path $InstallDir "l337-audio-server.exe")
    Set-ItemProperty -Path $ArpPath -Name "EstimatedSize" -Value ((Get-Item (Join-Path $InstallDir "l337-audio-server.exe")).Length / 1KB)
    Write-Ok "Registered in Add/Remove Programs"
}

function Remove-ArpEntry {
    Write-Info "Removing Add/Remove Programs entry..."
    if (Test-Path $ArpPath) {
        Remove-Item -Path $ArpPath -Force | Out-Null
        Write-Ok "ARP entry removed"
    } else {
        Write-Warn "No ARP entry found"
    }
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if ($DryRun) {
    Write-Info "Dry-run mode — would perform the following actions:"
    if ($Uninstall) {
        Write-Info "  Stop and remove service: $ServiceName"
        Write-Info "  Remove Add/Remove Programs entry"
        Write-Info "  Remove: $InstallDir"
        if ($RemoveData) {
            Write-Info "  Remove data: $DataDir"
        } else {
            Write-Info "  Retain data: $DataDir"
        }
    } else {
        Write-Info "  Query GitHub for $($PreRelease ? 'latest prerelease' : 'latest stable release')"
        Write-Info "  Download Windows binary to: $InstallDir\l337-audio-server.exe"
        Write-Info "  Create service wrapper: $InstallDir\$WrapperName"
        Write-Info "  Install service: $ServiceName"
        Write-Info "  Create data directory: $DataDir"
        Write-Info "  Register in Add/Remove Programs"
    }
    exit 0
}

if ($Uninstall) {
    Uninstall-Service
    Remove-ArpEntry
    if (Test-Path $InstallDir) {
        Write-Info "Removing installation directory: $InstallDir"
        Remove-Item -Path $InstallDir -Recurse -Force
    }
    if ($RemoveData) {
        if (Test-Path $DataDir) {
            Write-Info "Removing data directory: $DataDir"
            Remove-Item -Path $DataDir -Recurse -Force
        }
        Write-Ok "Data directories removed"
    } else {
        Write-Warn "Data directories retained:"
        Write-Warn "  $DataDir"
        Write-Warn "Re-run with -RemoveData to delete them."
    }
    Write-Ok "Uninstallation complete"
    exit 0
}

Write-Info "Checking for latest release..."
$endpoint = if ($PreRelease) { "releases?per_page=100" } else { "releases/latest" }
$release = Get-GithubRelease -Endpoint $endpoint
$tag = $release.tag_name
$published = $release.published_at
if (-not $tag) {
    Write-Fail "Could not determine latest release tag"
}
Write-Info "Latest release: $tag (published: $published)"

$assetName = Get-AssetName
$downloadUrl = "https://github.com/$Repo/releases/download/$tag/$assetName"
Write-Info "Selected asset: $assetName"

$installedVersion = ""
if (Test-Path (Join-Path $InstallDir "l337-audio-server.exe")) {
    Write-Info "Existing installation found at: $InstallDir\l337-audio-server.exe"
    try {
        $versionOutput = & (Join-Path $InstallDir "l337-audio-server.exe") --version 2>&1
        $installedVersion = ($versionOutput -split '\s+')[-1]
    } catch {
        Write-Warn "Could not determine installed version; will reinstall"
    }
    if ($installedVersion) {
        Write-Info "Installed version: $installedVersion"
        if ($installedVersion -eq $tag -and -not $Force) {
            Write-Ok "Installed binary is up-to-date ($installedVersion)"
            Write-Host ""
            Write-Host "To reinstall anyway, run with -Force:" -ForegroundColor Yellow
            Write-Host "  .\install.ps1 -Force $($PreRelease ? '-PreRelease ' : '')" -ForegroundColor Yellow
            exit 0
        }
    }
}

if (-not (Test-Path $InstallDir)) {
    Write-Info "Creating installation directory: $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$dest = Join-Path $InstallDir "l337-audio-server.exe"
Download-Binary -Url $downloadUrl -Dest $dest

if (-not (Test-Path $DataDir)) {
    Write-Info "Creating data directory: $DataDir"
    New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
}

$token = New-DefaultToken
$wrapper = New-ServiceWrapper -InstallDir $InstallDir -Token $token

Install-Service -WrapperPath $wrapper
New-ArpEntry -Version $tag

Write-Ok "Installation complete"
Write-Host ""
Write-Host "Next steps:" -ForegroundColor White
Write-Host "  Check status:    Get-Service -Name $ServiceName" -ForegroundColor Gray
Write-Host "  View logs:       Get-Content -Path `"$DataDir\$ServiceName.log`" -Wait" -ForegroundColor Gray
Write-Host "  Uninstall:       .\install.ps1 -Uninstall" -ForegroundColor Gray
Write-Host "  Or use Add/Remove Programs in Windows Settings" -ForegroundColor Gray
Write-Host ""
Write-Host "=========================================" -ForegroundColor White
Write-Host " Server Token" -ForegroundColor White
Write-Host "=========================================" -ForegroundColor White
Write-Host ""
Write-Host "  $token" -ForegroundColor Yellow
Write-Host ""
Write-Host "Add this token to your client configuration." -ForegroundColor White
Write-Host "=========================================" -ForegroundColor White
