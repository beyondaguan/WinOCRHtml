<#
.SYNOPSIS
  Stop winocr-host.exe and rebuild it without "Access denied (os error 5)".

.DESCRIPTION
  cargo build fails to replace target\release\winocr-host.exe while the file is
  locked by a running host process. The browser extension auto-restarts the host
  every ~3 seconds, so killing the process alone is not enough.

  This script:
    1. Kills all running winocr-host.exe processes
    2. Temporarily renames the NativeMessagingHosts registry keys
       (Chrome + Edge) so the browser cannot restart the host mid-build
    3. Runs: cargo build --release --bin winocr-host   (skip with -NoBuild)
    4. ALWAYS restores the registry keys (try/finally), even on build failure

.PARAMETER NoBuild
  Only stop the host and pause registration; do not build.

.PARAMETER RestoreOnly
  Only restore previously paused registration keys; do not kill or build.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File stop_host.ps1

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File stop_host.ps1 -NoBuild

.NOTES
  Pure ASCII + CRLF. If the script is interrupted and registration was not
  restored, restore manually:
    Rename-Item "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.winocr.native_host.stopbak" "com.winocr.native_host"
    Rename-Item "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.winocr.native_host.stopbak" "com.winocr.native_host"
#>
[CmdletBinding()]
param(
    [switch]$NoBuild,
    [switch]$RestoreOnly
)

$ErrorActionPreference = "Continue"

# Native messaging host registration keys (Chrome + Edge), see install_host.ps1
$regParentPaths = @(
    "HKCU:\Software\Google\Chrome\NativeMessagingHosts",
    "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts"
)
$regKeyName = "com.winocr.native_host"
$regBakName = "com.winocr.native_host.stopbak"

function Stop-HostProcesses {
    $procs = Get-Process -Name "winocr-host" -ErrorAction SilentlyContinue
    if (-not $procs) {
        Write-Host "[+] no winocr-host.exe is running"
        return $true
    }
    foreach ($p in $procs) {
        Write-Host ("[-] killing winocr-host.exe (PID {0})" -f $p.Id)
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
        } catch {
            Write-Host ("    FAILED to kill PID {0}: {1}" -f $p.Id, $_.Exception.Message) -ForegroundColor Red
        }
    }
    Start-Sleep -Milliseconds 800
    $left = Get-Process -Name "winocr-host" -ErrorAction SilentlyContinue
    if ($left) {
        Write-Host "[!] winocr-host.exe is still running." -ForegroundColor Red
        Write-Host "    Close the browser (or disable the extension), then run this script again."
        return $false
    }
    Write-Host "[+] all winocr-host.exe processes stopped"
    return $true
}

function Pause-HostRegistration {
    $paused = @()
    foreach ($parent in $regParentPaths) {
        $key = Join-Path $parent $regKeyName
        $bak = Join-Path $parent $regBakName
        if (-not (Test-Path $key)) { continue }
        if (Test-Path $bak) {
            try { Remove-Item -Path $bak -Recurse -Force -ErrorAction Stop } catch {}
        }
        try {
            Rename-Item -Path $key -NewName $regBakName -ErrorAction Stop
            $paused += $parent
            Write-Host ("[-] registration paused: {0}" -f $key)
        } catch {
            Write-Host ("    could not pause {0}: {1}" -f $key, $_.Exception.Message) -ForegroundColor Yellow
        }
    }
    if ($paused.Count -eq 0) {
        Write-Host "[*] no registration keys found (browser not installed?)"
    }
    return $paused
}

function Restore-HostRegistration($paused) {
    foreach ($parent in $paused) {
        $key = Join-Path $parent $regKeyName
        $bak = Join-Path $parent $regBakName
        if (-not (Test-Path $bak)) { continue }
        try {
            Rename-Item -Path $bak -NewName $regKeyName -ErrorAction Stop
            Write-Host ("[+] registration restored: {0}" -f $key)
        } catch {
            Write-Host ("    could not restore {0}: {1}" -f $key, $_.Exception.Message) -ForegroundColor Red
            Write-Host ("    restore manually: Rename-Item `"{0}`" `"{1}`"" -f $bak, $regKeyName)
        }
    }
}

# ---- main ----
Write-Host "=== WinOCR host stop/rebuild helper ==="

if ($RestoreOnly) {
    $paused = @()
    foreach ($parent in $regParentPaths) {
        if (Test-Path (Join-Path $parent $regBakName)) { $paused += $parent }
    }
    if ($paused.Count -eq 0) {
        Write-Host "[*] nothing to restore (no .stopbak keys found)"
        exit 0
    }
    Restore-HostRegistration $paused
    Write-Host "[+] done."
    exit 0
}

if ($PSScriptRoot) {
    Set-Location $PSScriptRoot
    Write-Host ("[*] working directory: {0}" -f (Get-Location))
}

$stopped = Stop-HostProcesses
if (-not $stopped) { exit 1 }

$paused = Pause-HostRegistration
try {
    if ($NoBuild) {
        Write-Host "[*] -NoBuild: registration stays paused."
        Write-Host "    Build manually, then run: powershell -ExecutionPolicy Bypass -File stop_host.ps1 -RestoreOnly"
        exit 0
    }
    Write-Host "[*] building: cargo build --release --bin winocr-host ..."
    & cargo build --release --bin winocr-host
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        Write-Host ("[!] build FAILED (exit {0})" -f $code) -ForegroundColor Red
        Write-Host "    If the error is still 'access denied', close the browser completely and retry."
        exit $code
    }
    Write-Host "[+] build OK"
} finally {
    Restore-HostRegistration $paused
}

Write-Host ""
Write-Host "[+] done. Reload the extension at chrome://extensions (or wait ~3s for auto-reconnect)."
