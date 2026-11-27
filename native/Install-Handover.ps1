# Attended, reversible cutover. This never uninstalls either rollback app.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$handoverRoot = Join-Path $env:LOCALAPPDATA 'Programs/CoolingLighting/Data'
$handoverPreviousRoot = Join-Path $env:LOCALAPPDATA 'CoolingLighting'
$handoverExe = Join-Path $env:LOCALAPPDATA 'Programs/CoolingLighting/CoolingLighting.exe'
$handoverReceipt = Join-Path $handoverRoot 'cutover-result.json'
$handoverBackup = Join-Path $handoverRoot ('cutover-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$handoverStartup = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs/Startup/RGB Tray Controller.lnk'
$handoverDesktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Cooling & Lighting.lnk'
$handoverLegacy = 'C:\Program Files (x86)\FanControl\FanControl.exe'
$handoverOldMain = 'C:\Users\lukem\source\repos\WIP\rgb-tray-app\main.py'
$handoverTaskCreated = $false
$handoverOldDisabled = $false
$handoverRgbStopped = $false
$handoverFanStopped = $false
$handoverShortcutMoved = $false
$handoverDesktopChanged = $false
$handoverTaskXml = $null
$handoverStarted = [DateTimeOffset]::UtcNow
function Invoke-HandoverNative([string]$Argument, [int]$Timeout = 90) {
    $invocation = Start-Process -FilePath $handoverExe -ArgumentList $Argument -WindowStyle Hidden -PassThru
    if (-not $invocation.WaitForExit($Timeout * 1000)) { throw "Native command is still running: $Argument. No forced termination was attempted." }
    if ($invocation.ExitCode -ne 0) { throw "Native command failed: $Argument" }
}
try {
    if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrator approval is required' }
    if (-not (Test-Path -LiteralPath $handoverExe)) { throw 'Installed native executable missing' }
    if (Get-Process CoolingLighting -ErrorAction SilentlyContinue) { throw 'Quit the native preview before cutover' }
    $existingNativeTask = Get-ScheduledTask -TaskName CoolingLighting-Native -ErrorAction SilentlyContinue
    if ($existingNativeTask) {
        # Only retire our exact diagnostic task; never replace a different action.
        if (@($existingNativeTask.Actions).Count -ne 1 -or $existingNativeTask.Actions[0].Execute -ne $handoverExe -or $existingNativeTask.State -eq 'Running') { throw 'Unexpected or running native task; refusing to overwrite' }
        Unregister-ScheduledTask -TaskName CoolingLighting-Native -Confirm:$false
    }
    if (-not (Test-Path -LiteralPath (Join-Path $handoverRoot 'settings.json'))) {
        [void](New-Item -ItemType Directory -Path $handoverRoot -Force)
        Copy-Item -LiteralPath (Join-Path $handoverPreviousRoot 'settings.json') -Destination (Join-Path $handoverRoot 'settings.json')
        Copy-Item -LiteralPath (Join-Path $handoverPreviousRoot 'settings.json.bak') -Destination (Join-Path $handoverRoot 'settings.json.bak')
    }
    $oldTask = Get-ScheduledTask -TaskName FanControl
    $handoverTaskXml = Export-ScheduledTask -TaskName FanControl
    [void](New-Item -ItemType Directory -Path $handoverBackup)
    [IO.File]::WriteAllText((Join-Path $handoverBackup 'FanControl-task.xml'), $handoverTaskXml)
    Copy-Item -LiteralPath (Join-Path $handoverRoot 'settings.json') -Destination (Join-Path $handoverBackup 'native-settings.json')
    Copy-Item -LiteralPath 'C:\Program Files (x86)\FanControl\Configurations\userConfig.json' -Destination (Join-Path $handoverBackup 'FanControl-userConfig.json')
    Copy-Item -LiteralPath (Join-Path $env:LOCALAPPDATA 'RGBTrayController/config.json') -Destination (Join-Path $handoverBackup 'RGB-config.json')
    if (Test-Path -LiteralPath $handoverDesktop) { Copy-Item -LiteralPath $handoverDesktop -Destination (Join-Path $handoverBackup 'Desktop-original.lnk') }
    # Only these exact RGB app processes are stopped; this app never writes fans.
    $rgbProcesses = @(Get-CimInstance Win32_Process -Filter "Name='pythonw.exe'" | Where-Object { $_.CommandLine -like ('*"' + $handoverOldMain + '"*') })
    foreach ($rgbProcess in $rgbProcesses) {
        Stop-Process -Id $rgbProcess.ProcessId
        $handoverRgbStopped = $true
    }
    # Fan Control's documented --exit performs its normal hardware handoff.
    $fanExit = Start-Process -FilePath $handoverLegacy -ArgumentList '--exit' -WindowStyle Hidden -PassThru
    [void]$fanExit.WaitForExit(10000)
    foreach ($fanProcess in @(Get-Process FanControl -ErrorAction SilentlyContinue)) { [void]$fanProcess.WaitForExit(20000) }
    if (Get-Process FanControl -ErrorAction SilentlyContinue) { throw 'Fan Control did not exit; no fan writes were made' }
    $handoverFanStopped = $true
    Invoke-HandoverNative '--handover-check'
    $check = Get-Content -LiteralPath (Join-Path $handoverRoot 'handover-check.json') -Raw | ConvertFrom-Json
    if (-not $check.Passed -or [DateTimeOffset]$check.Timestamp -lt $handoverStarted) { throw 'Fresh physical handover check did not pass' }
    Invoke-HandoverNative '--startup-check' 20
    Invoke-HandoverNative '--startup-export' 20
    $startupXml = Get-Content -LiteralPath (Join-Path $handoverRoot 'startup-plan.xml') -Raw -Encoding UTF8
    [void](Register-ScheduledTask -TaskName CoolingLighting-Native -Xml $startupXml)
    $handoverTaskCreated = $true
    [void](Disable-ScheduledTask -TaskName FanControl)
    $handoverOldDisabled = $true
    if (Test-Path -LiteralPath $handoverStartup) {
        # Literal, verified shortcut moved into a unique backup, not deleted.
        Move-Item -LiteralPath $handoverStartup -Destination (Join-Path $handoverBackup 'RGB-startup.lnk')
        $handoverShortcutMoved = $true
    }
    $shortcutShell = New-Object -ComObject WScript.Shell
    $shortcut = $shortcutShell.CreateShortcut($handoverDesktop)
    $shortcut.TargetPath = $handoverExe
    $shortcut.Arguments = '--launch-startup'
    $shortcut.WorkingDirectory = Split-Path $handoverExe
    $shortcut.Description = 'Cooling & Lighting - native cooling and RGB'
    $shortcut.Save()
    $handoverDesktopChanged = $true
    $startupTestTime = [DateTimeOffset]::UtcNow
    Start-ScheduledTask -TaskName CoolingLighting-Native
    $startupVerified = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Seconds 1
        try {
            $snapshot = Get-Content -LiteralPath (Join-Path $handoverRoot 'hardware-status.json') -Raw | ConvertFrom-Json
            if ([DateTimeOffset]$snapshot.Timestamp -ge $startupTestTime -and $snapshot.Administrator -and $snapshot.Status -in @('Cooling & lighting ready', 'Native hardware engine active') -and @($snapshot.Errors).Count -eq 0) { $startupVerified = $true; break }
        } catch { }
    }
    if (-not $startupVerified) {
        $taskInfo = Get-ScheduledTaskInfo -TaskName CoolingLighting-Native
        $taskInfo | Select-Object LastRunTime,LastTaskResult | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $handoverBackup 'startup-failure.json')
        throw ('Elevated scheduled launch could not be verified; task result ' + $taskInfo.LastTaskResult)
    }
    Invoke-HandoverNative '--launch-startup' 20
    [ordered]@{Success=$true; Timestamp=[DateTimeOffset]::UtcNow; Backup=$handoverBackup; Task='CoolingLighting-Native'; FanModes='Firmware (preserved unassigned legacy curves)'; RebootVerified=$false; OldAppsUninstalled=$false} | ConvertTo-Json | Set-Content -LiteralPath $handoverReceipt
} catch {
    $cutoverError = $_.Exception.ToString()
    $rollbackErrors = @()
    try {
        if ($handoverTaskCreated) {
            Invoke-HandoverNative '--quit' 20
            foreach ($nativeProcess in @(Get-Process CoolingLighting -ErrorAction SilentlyContinue)) { if (-not $nativeProcess.WaitForExit(20000)) { throw 'Native process did not release; no forced termination' } }
            Unregister-ScheduledTask -TaskName CoolingLighting-Native -Confirm:$false
        }
        if ($handoverOldDisabled) { [void](Register-ScheduledTask -TaskName FanControl -Xml $handoverTaskXml -Force) }
        if ($handoverShortcutMoved) { Move-Item -LiteralPath (Join-Path $handoverBackup 'RGB-startup.lnk') -Destination $handoverStartup }
        if ($handoverDesktopChanged -and (Test-Path -LiteralPath (Join-Path $handoverBackup 'Desktop-original.lnk'))) { Copy-Item -LiteralPath (Join-Path $handoverBackup 'Desktop-original.lnk') -Destination $handoverDesktop -Force }
        if ($handoverFanStopped -and -not (Get-Process FanControl -ErrorAction SilentlyContinue)) { Start-ScheduledTask -TaskName FanControl }
        if ($handoverRgbStopped) { Start-Process -FilePath 'C:\Users\lukem\source\repos\WIP\rgb-tray-app\.venv\Scripts\pythonw.exe' -ArgumentList ('"' + $handoverOldMain + '" --show') -WorkingDirectory (Split-Path $handoverOldMain) -WindowStyle Hidden }
    } catch { $rollbackErrors += $_.Exception.ToString() }
    [ordered]@{Success=$false; Timestamp=[DateTimeOffset]::UtcNow; Error=$cutoverError; RollbackErrors=$rollbackErrors; Backup=$handoverBackup} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $handoverReceipt
    exit 1
}
