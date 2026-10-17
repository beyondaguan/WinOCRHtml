# ============================================================
# WinOCR Native Messaging Host - install script
# Usage: right-click "Run with PowerShell", or:
#   powershell -ExecutionPolicy Bypass -File install_host.ps1
#
# What it does:
#   1. Generate host manifest (com.winocr.native_host.json) in repo
#   2. Write HKCU registry entries (no admin required)
#   3. Register for both Chrome and Edge NativeMessagingHosts
# ============================================================

$ErrorActionPreference = "Stop"

# ---- Paths ----
# Note: this project is a cargo workspace (root at repo root),
# so target output goes to <repo>\target\ regardless of cwd.
$RepoRoot = Split-Path -Parent $PSScriptRoot
# Use the release build (faster OCR; debug lacks some newer actions).
$HostExe  = Join-Path $RepoRoot "target\release\winocr-host.exe"
$ManifestDir = Join-Path $RepoRoot "browser-extension"
$ManifestFile = Join-Path $ManifestDir "com.winocr.native_host.json"
$HostName = "com.winocr.native_host"

# ---- Extension ID ----
# Default: the real extension ID (see chrome://extensions).
# Override with: $env:WINOCR_EXT_ID = "..." before running.
$ExtId = $env:WINOCR_EXT_ID
if (-not $ExtId) {
    $ExtId = "ahbpjindiehpcfpmjcfkpaigjfhnibji"
}
$AllowedOrigin = "chrome-extension://$ExtId/"

# ---- Check host exe exists ----
if (-not (Test-Path $HostExe)) {
    Write-Host "[X] winocr-host.exe not found: $HostExe"
    Write-Host "    Build it first:"
    Write-Host "      cd /d D:\AI\WinOCRHtml20060927"
    Write-Host "      cargo build -p winocr --bin winocr-host --release"
    exit 1
}

# ---- Generate manifest ----
$manifest = @{
    name         = $HostName
    description  = "WinOCR Native Messaging Host (word translation / vocabulary collection)"
    path         = $HostExe
    type         = "stdio"
    allowed_origins = @($AllowedOrigin)
} | ConvertTo-Json

# Write UTF-8 without BOM (Chrome requires valid UTF-8 JSON).
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($ManifestFile, $manifest, $utf8NoBom)
Write-Host "[+] Manifest written: $ManifestFile"
Write-Host "    allowed_origins: $AllowedOrigin"

# ---- Register in HKCU (Chrome and Edge share the same key layout) ----
$chromeKey = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$HostName"
$edgeKey   = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\$HostName"

foreach ($key in @($chromeKey, $edgeKey)) {
    if (-not (Test-Path $key)) {
        New-Item -Path $key -Force | Out-Null
    }
    Set-ItemProperty -Path $key -Name "(default)" -Value $ManifestFile
    Write-Host "[+] Registered: $key -> $ManifestFile"
}

Write-Host ""
Write-Host "[OK] WinOCR Native Messaging Host installed."
Write-Host "     Reload the extension (chrome://extensions -> refresh) to use it."
Write-Host ""
Write-Host "To uninstall, run uninstall_host.ps1"