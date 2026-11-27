#Requires -Version 5.1
<#
.SYNOPSIS
  Installs Stellar and hands cooling and lighting over from Cooling & Lighting.

.DESCRIPTION
  Run elevated (the script asks for elevation itself). It:
    1. builds nothing — it installs the files in -Source (see README: build.ps1);
    2. asks Cooling & Lighting to quit gracefully, which returns its fans to
       firmware control (never force-killed), then disables its startup task
       and archives its desktop shortcut so it can be restored;
    3. copies Stellar to %LOCALAPPDATA%\Programs\Stellar;
    4. registers the elevated "Stellar" logon task and Desktop/Start shortcuts;
    5. starts Stellar.
  restore-legacy.ps1 undoes the handover.
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\dist\Stellar'),
    [switch]$NoStart
)
$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    # Elevated processes cannot see drives mapped by the normal user (for
    # example a network share), so stage the installer and build locally first.
    $stage = Join-Path $env:TEMP "stellar-install-$(Get-Date -Format 'yyyyMMddHHmmss')"
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item $PSCommandPath (Join-Path $stage 'install.ps1')
    Copy-Item $Source (Join-Path $stage 'dist') -Recurse
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$(Join-Path $stage 'install.ps1')`"", '-Source', "`"$(Join-Path $stage 'dist')`"")
    if ($NoStart) { $argList += '-NoStart' }
    Write-Host 'Approve the Windows prompt to install Stellar...'
    $p = Start-Process powershell -Verb RunAs -ArgumentList $argList -Wait -PassThru
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    if ($p.ExitCode -ne 0) { Write-Host "Install did not complete (exit code $($p.ExitCode)). See the log in $env:LOCALAPPDATA\Programs\Stellar\Data\handover-backup-*." -ForegroundColor Red; exit 1 }
    Write-Host 'Stellar installed and started.' -ForegroundColor Green
    exit 0
}
trap {
    Write-Host "`nInstall failed: $_" -ForegroundColor Red
    if ($log) { Add-Content $log "FAILED: $_" }
    Read-Host 'Press Enter to close'
    exit 1
}

$dest = Join-Path $env:LOCALAPPDATA 'Programs\Stellar'
$data = Join-Path $dest 'Data'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backup = Join-Path $data "handover-backup-$stamp"
$log = New-Item -ItemType Directory -Force $backup | Join-Path -ChildPath 'install.log'
function Say($m) { $line = "$(Get-Date -Format 'HH:mm:ss') $m"; Write-Host $line; Add-Content $log $line }

if (-not (Test-Path (Join-Path $Source 'stellar.exe')) -or -not (Test-Path (Join-Path $Source 'hw\stellar-hw.exe'))) {
    throw "Build output not found in $Source (expected stellar.exe and hw\stellar-hw.exe)."
}

# 1. Stop a running Stellar gracefully so its files can be replaced.
$running = Get-Process stellar -ErrorAction SilentlyContinue
if ($running) {
    Say 'Asking the running Stellar to quit'
    & (Join-Path $Source 'stellar.exe') --quit
    $running | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
    if (Get-Process stellar -ErrorAction SilentlyContinue) { throw 'Stellar did not quit; close it from the tray and run again.' }
}

# 2. Retire Cooling & Lighting (kept installed for rollback).
$legacyExe = Join-Path $env:LOCALAPPDATA 'Programs\CoolingLighting\CoolingLighting.exe'
$legacyTask = Get-ScheduledTask -TaskName 'CoolingLighting-Native' -ErrorAction SilentlyContinue
if ($legacyTask) {
    Export-ScheduledTask -TaskName 'CoolingLighting-Native' | Set-Content (Join-Path $backup 'CoolingLighting-Native.xml') -Encoding Unicode
}
if (Get-Process CoolingLighting -ErrorAction SilentlyContinue) {
    Say 'Asking Cooling & Lighting to quit (its fans return to firmware control)'
    if (Test-Path $legacyExe) { & $legacyExe --quit }
    Get-Process CoolingLighting -ErrorAction SilentlyContinue | Wait-Process -Timeout 30 -ErrorAction SilentlyContinue
    if (Get-Process CoolingLighting -ErrorAction SilentlyContinue) {
        throw 'Cooling & Lighting did not quit. Quit it from its tray icon, then run this again. (It is never force-killed because it owns fan outputs.)'
    }
}
if ($legacyTask) {
    Stop-ScheduledTask -TaskName 'CoolingLighting-Native' -ErrorAction SilentlyContinue
    Disable-ScheduledTask -TaskName 'CoolingLighting-Native' | Out-Null
    Say 'Disabled the CoolingLighting-Native startup task'
}
foreach ($desk in @([Environment]::GetFolderPath('Desktop'), (Join-Path $env:USERPROFILE 'Desktop'))) {
    $lnk = Join-Path $desk 'Cooling & Lighting.lnk'
    if (Test-Path $lnk) {
        Move-Item $lnk (Join-Path $backup 'Cooling & Lighting.lnk') -Force
        Set-Content (Join-Path $backup 'shortcut-location.txt') $desk
        Say "Archived $lnk"
    }
}
if (Get-Process FanControl -ErrorAction SilentlyContinue) {
    Say 'Note: Fan Control is running. Stellar keeps fans on firmware control until it exits.'
}

# 3. Install files.
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $Source 'stellar.exe') $dest -Force
if (Test-Path (Join-Path $dest 'hw')) { Remove-Item (Join-Path $dest 'hw') -Recurse -Force }
Copy-Item (Join-Path $Source 'hw') (Join-Path $dest 'hw') -Recurse -Force
Say "Installed to $dest"

# 4. Elevated logon task + shortcuts.
$exe = Join-Path $dest 'stellar.exe'
$user = "$env:USERDOMAIN\$env:USERNAME"
$action = New-ScheduledTaskAction -Execute $exe -Argument '--tray' -WorkingDirectory $dest
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -StartWhenAvailable
Register-ScheduledTask -TaskName 'Stellar' -Action $action -Trigger $trigger -Principal $taskPrincipal -Settings $settings `
    -Description 'Stellar cooling & lighting (elevated, starts at sign-in)' -Force | Out-Null
Say 'Registered the elevated Stellar logon task'

$shell = New-Object -ComObject WScript.Shell
foreach ($dir in @([Environment]::GetFolderPath('Desktop'), (Join-Path ([Environment]::GetFolderPath('Programs')) ''))) {
    $s = $shell.CreateShortcut((Join-Path $dir 'Stellar.lnk'))
    $s.TargetPath = $exe
    $s.WorkingDirectory = $dest
    $s.IconLocation = "$exe,0"
    $s.Description = 'Stellar cooling & lighting'
    $s.Save()
}
Say 'Created Desktop and Start menu shortcuts'

# 5. Start.
if (-not $NoStart) {
    Start-ScheduledTask -TaskName 'Stellar'
    Say 'Started Stellar'
}
Say "Done. Rollback: restore-legacy.ps1 (backup: $backup)"
