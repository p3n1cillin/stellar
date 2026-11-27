#Requires -Version 5.1
<#
.SYNOPSIS
  Hands control back from Stellar to the previous Cooling & Lighting app.
  Stellar stays installed but its startup task is disabled.
#>
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    # Stage locally: elevated processes cannot see user-mapped network drives.
    $local = Join-Path $env:TEMP 'stellar-restore-legacy.ps1'
    Copy-Item $PSCommandPath $local -Force
    Start-Process powershell -Verb RunAs -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$local`"") -Wait
    exit
}
trap {
    Write-Host "`nRestore failed: $_" -ForegroundColor Red
    Read-Host 'Press Enter to close'
    exit 1
}
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Stellar'
$exe = Join-Path $dest 'stellar.exe'

# Graceful quit returns Stellar's fans to firmware control.
if (Get-Process stellar -ErrorAction SilentlyContinue) {
    & $exe --quit
    Get-Process stellar -ErrorAction SilentlyContinue | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
    if (Get-Process stellar -ErrorAction SilentlyContinue) { throw 'Stellar did not quit; quit it from its tray icon and run again.' }
}
Disable-ScheduledTask -TaskName 'Stellar' -ErrorAction SilentlyContinue | Out-Null
Remove-Item (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Stellar.lnk') -ErrorAction SilentlyContinue

$latest = Get-ChildItem (Join-Path $dest 'Data') -Directory -Filter 'handover-backup-*' -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -Last 1
if ($latest) {
    $lnk = Join-Path $latest.FullName 'Cooling & Lighting.lnk'
    $where = Join-Path $latest.FullName 'shortcut-location.txt'
    if ((Test-Path $lnk) -and (Test-Path $where)) { Copy-Item $lnk (Join-Path (Get-Content $where) 'Cooling & Lighting.lnk') -Force }
}
Enable-ScheduledTask -TaskName 'CoolingLighting-Native' | Out-Null
Start-ScheduledTask -TaskName 'CoolingLighting-Native'
Write-Host 'Cooling & Lighting restored and started. Stellar is installed but disabled.'
