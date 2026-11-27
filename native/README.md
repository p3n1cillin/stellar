# Native Cooling & Lighting replacement

This is the native replacement for the Python RGB tray app and Fan
Control. It contains its own fan backend; it does not call Fan Control to set
fan speeds. The installed app and elevated startup task are active. Imported
channels remain in firmware mode; real Windows restart/suspend checks are pending.

## Run and verify

This PC already has the required .NET 8 desktop runtime and PawnIO installation.

```powershell
dotnet build native/CoolingLighting.csproj -c Release -o native/bin/staged
dotnet native/bin/staged/CoolingLighting.dll --self-test
dotnet native/bin/staged/CoolingLighting.dll --probe
dotnet native/bin/staged/CoolingLighting.dll --rgb-probe
dotnet native/bin/staged/CoolingLighting.dll --import-probe
dotnet native/bin/staged/CoolingLighting.dll --startup-check
```

The last three commands are read-only probes. `--import-probe` does not save,
back up, change or activate the installed Fan Control configuration.
`--startup-check` asks Windows Task Scheduler to validate the prospective XML
using `TASK_VALIDATE_ONLY`. It does not register, change or launch any task.
Validation alone does not install startup. `Install-Handover.ps1` performs the
explicit, attended migration with backups and automatic rollback on failed checks.

Run `native/bin/staged/CoolingLighting.exe` for the window, or pass `--tray`
for a quiet tray launch. Close hides the window; **Quit app** in Settings or the tray's
quit command releases owned cooling outputs to firmware and exits. Running the
app again raises the existing instance. Windows administrator approval is
required to access CPU/motherboard sensors and fan outputs; an assistant cannot
approve that security prompt for you.

## Implemented

- Redesigned charcoal/lime dashboard with separate Overview, Cooling, Lighting
  and Settings pages, live temperature traces, fan/device cards and an expandable
  sensor library. Curve points can be dragged; edits remain drafts until saved.
- Matching fan icon for the EXE, window, tray and desktop shortcut, with seven
  Windows icon sizes for high-DPI displays. Source artwork is in `Assets/App.svg`;
  `scripts/Build-Icon.ps1` regenerates the ICO and sidebar PNG deterministically.
  Migration and elevation buttons are removed from the normal UI. Settings now
  shows system status, storage, startup and tray behaviour; unused zero-RPM
  headers stay in the sensor library without cluttering the overview.
- Live hardware telemetry using LibreHardwareMonitor, temperature curves,
  manual and firmware modes, fan names, RPM pairing and curve preview.
  Multiple selected temperature sensors use the hottest reading. Missing saved
  temperature/RPM references remain visible instead of silently being replaced.
- Conservative minimum/startup speeds, immediate increases, hysteresis and
  rate-limited decreases. Manual mode retains temperature protection.
- Missing/stale/invalid temperature or paired RPM feedback requests full cooling.
  A paired fan below 100 RPM for 15 seconds while commanded above zero triggers
  stall protection. This is an alarm/fallback, not proof of a functional fan.
- Exclusive cooling ownership: outputs are blocked while Fan Control runs.
  Partial write failures attempt full cooling for owned outputs.
- Firmware handoff on normal exit, Windows session ending and suspend; resume
  clears policy history. These paths still need physical hardware verification.
- RGB presets/custom colour, on/off, saved LED counts and automatic restore/retry.
  OpenRGB remains a local background hardware backend, not a second control UI.
  The native app can start an installed SDK backend if none is running.
- The original RGB app's file lock prevents competing lighting writers. Native
  lighting takeover waits for the old app to exit; normal development does not
  stop it automatically.
- Immediate flushed atomic settings writes, a recovery copy, named profiles,
  and read-only legacy RGB migration. Corrupt copies default safely to firmware
  mode with a warning and remain untouched for recovery.
  A committed primary save remains authoritative if updating the backup fails;
  a persistent warning reports the missing recovery copy.
- Explicit metadata import backs up the original Fan Control JSON and preserves
  names, RPM pairings and calibration tables. New imports stay in firmware mode;
  it does not translate arbitrary legacy curves or assume saved settings equal
  the live configuration. Ambiguous GPU mappings are skipped.
- Local bounded logs and a hardware snapshot every ten seconds.

Settings live at `%LOCALAPPDATA%/Programs/CoolingLighting/Data/settings.json`, named profiles
under `profiles/`, import backups under `imports/`, diagnostics in `app.log` and
`hardware-status.json`. No network telemetry is sent.

This location avoids packaged-host AppData redirection: scheduled launches and
development launches now share the same settings and diagnostic files. Lighting
ownership checks guard both legacy lock locations and deduplicate actual files.

## Installed handover and remaining checks

On 3 October 2026 the attended hardware check and scheduled launch passed:

- All three imported outputs were commanded to 100%, RPM checked against saved
  calibration, then released and verified back at firmware duty cycles.
- The native lighting backend restored the saved white/on configuration.
- The scheduled task ran elevated with valid CPU/motherboard telemetry and no
  recorded hardware errors. A graceful close/relaunch retained all three rules.
- Fan Control's task is disabled, the RGB startup shortcut is archived, and the
  desktop shortcut launches the native task and opens its one existing instance.
- Old configurations/task XML/shortcuts are retained under the Data folder's
  `cutover-backup-*` directory. Neither old application has been uninstalled yet.

`Restore-Legacy.ps1`, run with administrator approval, restores the archived task
and shortcuts after requesting a graceful native quit. It never force-kills the
native fan controller. This is an explicit rollback tool, not an automatic monitor.

Before uninstalling the rollback application, confirm a real Windows restart,
lighting restoration and normal fan operation. Shutdown and suspend/resume also
remain to be checked. Physical thermal/stall emergency conditions were not
induced; their application-side policy is tested with fake hardware.
The ID-COOLING fan/pump lighting limitation is unchanged; there is no evidence
that those lights expose a software-controllable interface on this machine.

## Test scope

The polish review adds explicit save wording, a profile-name placeholder and
disabled invalid profile actions, per-output in-memory fan drafts with reset and
quit warnings, reliable CPU/GPU temperature defaults, missing-channel cards,
thread-safe UI error snapshots and a desktop-startup ready handshake. Existing
Firmware rules and lighting preferences are not migrated or changed by this review.
Drafts survive fan selection changes, not application exit; press Save to persist them.

`--self-test` runs 26 policy checks, 38 persistence/import checks, 22 fan
ownership/failure-path checks, 12 startup-plan checks, 6 curve-editor checks and
6 lighting-ownership checks, 6 branding-resource checks and 8 temperature-selection checks (124 total). Output tests use fake hardware; lighting
lease tests use temporary files only.
They demonstrate application-side decisions and release attempts, not actual
RPM changes, driver guarantees or the physical safety of a finished installation.

Startup API reference: [Microsoft's TaskFolder.RegisterTask documentation](https://learn.microsoft.com/en-us/windows/win32/taskschd/taskfolder-registertask).
