# Explicit rollback only. Run elevated if the user chooses to return to the old apps.
$ErrorActionPreference = 'Stop'
$restoreRoot = Join-Path $env:LOCALAPPDATA 'Programs/CoolingLighting/Data'
$restoreExe = Join-Path $env:LOCALAPPDATA 'Programs/CoolingLighting/CoolingLighting.exe'
$restoreReceipt = Get-Content -LiteralPath (Join-Path $restoreRoot 'cutover-result.json') -Raw | ConvertFrom-Json
$restoreBackup = [IO.Path]::GetFullPath($restoreReceipt.Backup)
$restorePrefix = [IO.Path]::GetFullPath($restoreRoot).TrimEnd('\') + '\'
if (-not $restoreBackup.StartsWith($restorePrefix, [StringComparison]::OrdinalIgnoreCase) -or
    -not ([IO.Path]::GetFileName($restoreBackup).StartsWith('cutover-backup-'))) { throw 'Unexpected backup path' }
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrator approval is required' }
$restoreTask = Get-ScheduledTask -TaskName CoolingLighting-Native -ErrorAction SilentlyContinue
if ($restoreTask -and (@($restoreTask.Actions).Count -ne 1 -or $restoreTask.Actions[0].Execute -ne $restoreExe)) { throw 'Unexpected native task action' }
$quitRequest = Start-Process -FilePath $restoreExe -ArgumentList '--quit' -WindowStyle Hidden -PassThru
[void]$quitRequest.WaitForExit(5000)
foreach ($restoreProcess in @(Get-Process CoolingLighting -ErrorAction SilentlyContinue)) {
    if (-not $restoreProcess.WaitForExit(20000)) { throw 'Native app has not released its outputs; rollback stopped without forcing termination' }
}
if ($restoreTask) { Unregister-ScheduledTask -TaskName CoolingLighting-Native -Confirm:$false }
$restoreXml = Get-Content -LiteralPath (Join-Path $restoreBackup 'FanControl-task.xml') -Raw
[void](Register-ScheduledTask -TaskName FanControl -Xml $restoreXml -Force)
$restoreStartup = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs/Startup/RGB Tray Controller.lnk'
if (Test-Path -LiteralPath (Join-Path $restoreBackup 'RGB-startup.lnk')) {
    if (Test-Path -LiteralPath $restoreStartup) { throw 'RGB startup shortcut already exists; not overwriting it' }
    Copy-Item -LiteralPath (Join-Path $restoreBackup 'RGB-startup.lnk') -Destination $restoreStartup
}
Copy-Item -LiteralPath (Join-Path $restoreBackup 'Desktop-original.lnk') -Destination (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Cooling & Lighting.lnk') -Force
Start-ScheduledTask -TaskName FanControl
Start-Process -FilePath 'C:\Users\lukem\source\repos\WIP\rgb-tray-app\.venv\Scripts\pythonw.exe' -ArgumentList '"C:\Users\lukem\source\repos\WIP\rgb-tray-app\main.py" --show' -WorkingDirectory 'C:\Users\lukem\source\repos\WIP\rgb-tray-app' -WindowStyle Hidden
