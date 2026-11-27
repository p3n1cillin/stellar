# Cooling & Lighting → Stellar

**Stellar** ([`stellar/`](stellar/README.md)) is the Rust successor to the app below. It manages every fan header, the GPU fan and all OpenRGB lighting. Its cooling works like Fan Control (reusable curves, start/stop speeds, step rates), and it adds auto-tuning, effects and automation. Install it with `stellar\build.ps1` and then `stellar\install.ps1`. `stellar\restore-legacy.ps1` rolls back to the app described below.

---

The independent native application is now installed at
`%LOCALAPPDATA%\Programs\CoolingLighting\CoolingLighting.exe`.
The desktop shortcut and `CoolingLighting-Native` scheduled task launch it.
The old Python app is stopped and its startup shortcut archived; Fan Control is
stopped and its task disabled. The old installation is retained for rollback
until a real Windows restart has been checked. See [`native/`](native/README.md).

Imported fans remain in firmware mode, preserving the legacy configuration's
unassigned curves. Full-speed actuation and return to firmware were checked on
both motherboard channels and the RTX 5070. Restart/suspend testing is pending.

## Legacy Python app (rollback only)

A shared Windows control centre for RGB lighting and your installed Fan Control.
Double-click the tray icon to open the dashboard. Run `python main.py --show`
to open the window immediately; without `--show` it starts quietly in the tray.
Opening it again raises the existing window rather than starting a second instance.

## Fan Control integration

Fan Control remains the cooling engine, with its own drivers, calibrations,
curves and Windows startup task. Its source is closed; this app integrates with
the installed program through its supported configuration command line.

The dashboard discovers installed cooling profiles and shows their saved fan
settings. Select a profile and choose **Apply profile** to send it to Fan Control.
Windows may request elevation because Fan Control runs as administrator. A sent
command is not confirmation of a profile change; verify live settings in the editor.
**Fan curves & live sensors** opens the installed editor for live temperatures,
RPM readings, curve editing and calibration. The dashboard table is saved profile
data, not live telemetry. **Save profile copy** and **Import profile** preserve
calibration and curves in `%LOCALAPPDATA%\RGBTrayController\FanProfiles` without
overwriting installed profiles. Fan Control owns persistence of its active cooling
configuration; RGB settings are saved separately by this app.

Fan Control is detected under Program Files; portable installs can set
`FANCONTROL_PATH` to the full path of `FanControl.exe`. If Fan Control isn't
running, the app tries the existing `FanControl` Windows scheduled task.
Quitting this app leaves Fan Control running so cooling continues.

Integration reference: [Fan Control's supported configuration and window commands](https://getfancontrol.com/docs/#command-line-arguments).

A small Windows system-tray app to turn your PC's RGB lighting on/off and set
its color, via [OpenRGB](https://openrgb.org) — free and open-source, works
across most RGB motherboards/RAM/GPUs/keyboards regardless of brand (no
vendor SDK, no account, no cloud).

## Setup

1. Install [OpenRGB](https://openrgb.org/) and launch it once so it detects
   your devices. The tray app will start OpenRGB's local SDK server when it
   is not already running. A non-standard install can be specified with the
   `OPENRGB_PATH` environment variable.
2. From this folder:
   ```
   python -m venv .venv
   .venv\Scripts\pip install -r requirements.txt
   ```
3. Run it:
   ```
   .venv\Scripts\python main.py
   ```
   Or, to run without a console window staying open:
   ```
   .venv\Scripts\pythonw main.py
   ```

## Use

A circular icon appears in the system tray:

- **Colored** = on, showing that color
- **Gray** = off
- **Red** = waiting for OpenRGB, a lighting command failed, or settings could
  not be saved; hover over the icon for status. Connection failures retry automatically.

Double-click opens Cooling & Lighting. Right-click for the full
menu: preset colors, **Custom Color...** (opens a color picker), reconnect,
and quit.

Your color, on/off state, and fan LED counts are saved immediately whenever
you change them, in `%LOCALAPPDATA%\RGBTrayController\config.json`. Writes
are flushed to disk and replaced atomically, with a backup for recovery.
Existing `config.json` settings next to `main.py` migrate automatically.
You do not need to quit the app before shutting down Windows.

At sign-in the app restores that state, retrying every five seconds if
OpenRGB is still starting. It also reconnects after an SDK server restart.
Only one tray instance can run at a time. Errors are recorded in
`%LOCALAPPDATA%\RGBTrayController\app.log`.

Controllers whose LED zones default to zero can also be configured there.
`zone_sizes` maps the OpenRGB device and zone names to their physical LED
counts; the app restores these sizes whenever the SDK server restarts.

## Notes

- "Off" uses each device's own hardware Off mode when it has one; devices
  that don't expose one (most peripherals only have Direct/Static/Effect
  modes) are set to black instead — still fully dark, just not power-off.
- Everything runs locally against `127.0.0.1:6742`; nothing leaves your
  machine.
- Not code-signed — Windows SmartScreen may warn on first run of a script
  from an unfamiliar source. It's a plain Python script; read `main.py`
  yourself if you want to verify that before running it.
