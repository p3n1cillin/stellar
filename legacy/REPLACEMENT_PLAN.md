# Native replacement completion gates

The requested outcome is one application replacing the Python RGB tray app and
Fan Control on this PC. A launcher or profile bridge is not completion.

## Required and verified before cutover

- Own hardware backend, independent of the Fan Control installation.
- Live CPU/GPU temperatures, motherboard fan RPM and RTX 5070 fan RPM.
- Independent motherboard and GPU fan outputs, tested against physical RPM.
- Editable temperature curves, manual and firmware modes, fan naming and pairing.
- Calibration preserved or reproduced, minimum speed, startup and hysteresis.
- Missing/stale sensor failsafe, overtemperature override, control ownership,
  release to firmware on exit, reliable shutdown and suspend/resume handling.
- Integrated RGB on/off, presets, custom colours, and remembered LED zone counts.
- Settings and named profiles survive restart, shutdown and migration.
- One desktop/tray UI, one product install, elevated unattended Windows startup.
- User's working settings backed up and imported; old apps remain operational
  during development, with no competing fan writers.
- A sustained hardware run, UI validation, startup/restart verification and
  meaningful engine tests pass before disabling old startup entries.

## Implementation

`native/` is the replacement, using the published open-source
LibreHardwareMonitorLib package, not proprietary Fan Control libraries.
The native application is now the active app; the Python and Fan Control
installations are retained as rollback only until Windows restart verification.
Hardware probes are read-only. Fan writes are gated by explicit rules and blocked
while Fan Control runs. The replacement is incomplete until every gate above has
authoritative runtime evidence.

## Current verification evidence

- Native build passes without warnings or errors.
- 26 cooling-policy checks, 38 persistence/import checks, 22 fake-output
  ownership/failure-path checks, 12 startup-plan checks, 6 curve-editor checks
  and 6 lighting-ownership checks pass; the existing
  Python app's 14 tests also pass.
- Read-only native probes detect the RTX 5070 temperature, fan RPM and control
  interface, plus ASUS, Corsair Lighting Node Core and Keychron RGB controllers.
- Native window visually checked: the GPU control defaults to its own core
  temperature and fan RPM, rather than the integrated GPU's temperature.
- Read-only legacy import resolves three visible fan records and preserves
  their saved calibration tables. Installed JSON SHA256 remains
  `9E11D8B36D9596A2BBB58B5A41731E4DC40740D068D7DDEAA4A9F974503568E5`.
- The native scheduled task is active, Fan Control's task is disabled, the RGB
  startup shortcut is archived and the desktop shortcut targets the native app.
- Fake-output tests cover non-admin and competing-writer gates, suspend/resume,
  removed rules, failed/partial writes, missing outputs, retrying failed releases,
  hardware speed bounds and startup-floor reapplication. They do not prove
  physical fan actuation or firmware handoff.
- Redesigned dashboard and Cooling/Lighting/Settings pages are visually checked
  in the running native app. Dragging curve points updates the draft graph and
  text without saving or applying a fan rule. Temperature mixes, missing saved
  sensor references and persistent save/import notices remain supported.
- Elevation handoff waits for the previous instance to release ownership instead
  of depending on an arbitrary 2.5-second delay.
- Windows Task Scheduler accepts the startup XML through `TASK_VALIDATE_ONLY`.
  The native task was subsequently registered and run successfully. Real sign-in
  testing remains pending. The plan targets one user's interactive tray session at highest
  available privileges, without a stored password or finite execution timeout.
- Named profile disk reload, independent copies, backup recovery, corrupt-copy
  refusal and filename/path safeguards are tested. Active profile names are
  persisted instead of always displaying the default label after a restart.

## Remaining hardware and startup verification

The user returned home and approved the attended cutover. The installed native
app runs from `%LOCALAPPDATA%/Programs/CoolingLighting`, with shared state in its
`Data` subfolder. Legacy motherboard control/RPM IDs now map
to detected native chip-instance IDs by exact chip/type/channel; ambiguous
instances are rejected. The updated administrator dashboard reads CPU and
motherboard telemetry. Metadata import preserved all three enabled controls.
The physical handover check commanded full speed, observed calibrated RPM on
both motherboard channels and the GPU, verified return to firmware, and restored
RGB. Cold scheduled launch and graceful close/relaunch passed. No legacy fan
curves were assigned, so the imported rules preserve firmware mode rather than
inventing a new cooling policy. Real Windows restart/shutdown/suspend verification
and final uninstall of the rollback application remain pending. This is not yet
a fully verified unattended installation.
