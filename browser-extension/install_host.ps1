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
# 两种布局自适应：
#   仓库布局（开发）：  <repo>\browser-extension\ + <repo>\target\release\winocr-host.exe
#   安装布局（Inno）：  <app>\browser-extension\ + <app>\winocr-host.exe（exe 在上级目录）
$ManifestDir = Join-Path (Split-Path -Parent $PSScriptRoot) "browser-extension"
if (-not (Test-Path $ManifestDir)) { $ManifestDir = $PSScriptRoot }
$ManifestFile = Join-Path $ManifestDir "com.winocr.native_host.json"
$HostName = "com.winocr.native_host"
$Candidates = @(
    (Join-Path $PSScriptRoot "..\winocr-host.exe"),                        # 安装布局
    (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\winocr-host.exe")  # 仓库布局
)
$HostExe = $Candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

# ---- Extension IDs ----
# 首选：钉死 ID（manifest.json 带 "key" 字段，解压加载的 ID 恒为 icfifim...，全机器通用）。
# 保留旧开发 ID 兼容；Override with: $env:WINOCR_EXT_ID = "id1,id2" (comma-separated) before running.
# Existing allowed_origins in the current manifest are preserved (merged),
# so re-running the installer never removes an ID already in use.
$ExtIds = @()
if ($env:WINOCR_EXT_ID) {
    $ExtIds = $env:WINOCR_EXT_ID -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
}
if ($ExtIds.Count -eq 0) {
    $ExtIds = @("icfifimpcloljgamnegphindmepjfmjl", "ahbpjindiehpcfpmjcfkpaigjfhnibji")
}
# Merge with existing manifest allowed_origins (if any) to avoid dropping IDs.
$ExistingOrigins = @()
if (Test-Path $ManifestFile) {
    try {
        $existing = Get-Content -LiteralPath $ManifestFile -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($existing.allowed_origins) {
            $ExistingOrigins = @($existing.allowed_origins | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        }
    } catch {
        Write-Host "[!] 无法读取现有 manifest，将仅使用本次提供的扩展 ID: $($_.Exception.Message)"
    }
}
$AllowedOrigins = @($ExistingOrigins)
foreach ($ExtId in $ExtIds) {
    $origin = "chrome-extension://$ExtId/"
    if ($AllowedOrigins -notcontains $origin) {
        $AllowedOrigins += $origin
    }
}

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
    allowed_origins = @($AllowedOrigins)
} | ConvertTo-Json

# Write UTF-8 without BOM (Chrome requires valid UTF-8 JSON).
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($ManifestFile, $manifest, $utf8NoBom)
Write-Host "[+] Manifest written: $ManifestFile"
Write-Host "    allowed_origins: $($AllowedOrigins -join ', ')"

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