# ============================================================
# WinOCR Native Messaging Host - uninstall script
# Usage: right-click "Run with PowerShell"
# Removes registry entries and host manifest
# ============================================================

$ErrorActionPreference = "Stop"

$HostName = "com.winocr.native_host"
$ManifestFile = Join-Path (Split-Path -Parent $PSScriptRoot) "browser-extension\com.winocr.native_host.json"

$chromeKey = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$HostName"
$edgeKey   = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\$HostName"

foreach ($key in @($chromeKey, $edgeKey)) {
    if (Test-Path $key) {
        Remove-Item -Path $key -Recurse -Force
        Write-Host "[-] Removed registry: $key"
    }
}

if (Test-Path $ManifestFile) {
    Remove-Item -Path $ManifestFile -Force
    Write-Host "[-] Removed manifest: $ManifestFile"
}

Write-Host "[OK] WinOCR Native Messaging Host uninstalled."