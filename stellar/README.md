# Stellar

Stellar is the Rust successor to Cooling & Lighting. One app now runs every fan header, the GPU fan and all OpenRGB lighting. Its cooling model works like Fan Control's, and it can tune your fans for you.

## Install

```powershell
.\build.ps1        # tests, release build, hardware host -> .\dist
.\install.ps1      # asks for elevation once
```

`install.ps1` does the following:

- Asks Cooling & Lighting to quit gracefully. Its fans return to firmware control, and it's never force-killed.
- Disables its `CoolingLighting-Native` task and archives its shortcut under `Data\handover-backup-*`.
- Installs Stellar to `%LOCALAPPDATA%\Programs\Stellar`.
- Registers an elevated `Stellar` logon task and adds Desktop and Start menu shortcuts.

`restore-legacy.ps1` hands control back to the old app.

Stellar always runs elevated. The shortcut starts it through the logon task, so there's no prompt and no limited "not administrator" mode. On first run it imports the settings and profiles from Cooling & Lighting, read-only.

## Tune your fans

Open **Cooling** and click **Auto-tune all fans**, or run `stellar --autotune` while Stellar is running. For each header it:

1. Sweeps the header from 100% down to 0%, holding each step until the RPM settles. A header with no RPM at 100% is empty and gets skipped.
2. Finds where the fan stalls and steps back up to find the speed that restarts it.
3. Finds the "PWM floor", the point below which a fan or hub ignores the duty cycle, and the point where extra speed stops adding much airflow.
4. Sets each fan's **minimum** and **start** speed.
   - On a fan hub, the RPM signal comes from only one fan, so an 8% margin covers the others.
   - A hub that never stops gets its floor as the minimum.
5. Creates tuned **CPU** and **GPU** curves and a **Case** mix curve (whichever of CPU and GPU is hotter).
   - The first motherboard header (normally CPU_FAN) follows CPU.
   - Every other header, including your hub, follows Case.
   - The GPU fan keeps its own firmware curve; its measurements are stored.

**AIO coolers.** Set **CPU cooler: AIO liquid** on the Cooling page. The coolant in an AIO lags the CPU, so Stellar gives the radiator fans:

- a gentler **CPU (AIO radiator)** curve with a 10-second **response time**, which smooths the input temperature so the fans follow the trend, not Tctl spikes;
- 4 °C of hysteresis and a slower step-up.

Changing the cooler type rebuilds the curves from the stored measurements; no new sweep is needed. **Rebuild curves** does the same at any time. Safety limits always use the raw, unsmoothed temperature.

Your previous setup is saved as the profile **Before auto-tune**. Tuning stops immediately and returns fans to normal control if the CPU or GPU reaches 85 °C or any safeguard fires.

## Cooling

- **Curves are reusable**, as in Fan Control:
  - **Graph**: draggable points over one or more sensors, using the hottest or the average, with an offset.
  - **Linear**, **Flat** and **Trigger** (idle/load with hysteresis).
  - **Mix**: highest, lowest, average or sum of other curves.
- **Per fan**: Firmware, Curve or Manual mode, plus:
  - minimum (stop) and start speeds, and an optional fan stop
  - offset, step-up and step-down rates, and hysteresis
  - RPM pairing and measured calibration
- **Safety** is always on, and rate limits never delay it. The fan goes to 100% instantly when:
  - any watched sensor reaches the critical limit
  - a sensor is missing or stale
  - RPM feedback is lost
  - the fan stalls (under 100 RPM for 15 s)
  - a write fails
- **Hardware host**: the `hw\stellar-hw.exe` LibreHardwareMonitor host returns every output it touched to firmware control when Stellar exits, crashes, or stops responding for 10 s.
- **Other fan software**: while Fan Control or Cooling & Lighting runs, Stellar leaves all fans on firmware control so two programs never fight over them.

## Lighting

- Stellar renders effects itself and streams them to every OpenRGB device: the ASUS Aura headers, the Corsair Lighting Node and the Keychron keyboard.
  - Effects: static, breathing, spectrum, rainbow wave, gradient, temperature-reactive (for example CPU temperature from blue to red), and off.
- Brightness, per-device on/off and per-device effects, zone LED counts, and quiet hours.
- It starts the installed OpenRGB server if needed, and won't fight the old RGB apps for the lighting lock.

## Profiles and automation

- Profiles store curves, fans and lighting.
- Automation switches profile while a program runs (for example a game), with an optional fallback profile.
- The tray menu switches profiles and toggles lighting. The tray icon turns red while a safeguard is active.

## Command line

```
stellar            open the dashboard (or raise the running instance)
stellar --tray     start in the tray
stellar --quit     return fans to firmware and exit
stellar --autotune tune every fan (running instance)
stellar status     one-off hardware reading (JSON)
stellar rgb        list OpenRGB devices
```

## Files

Data lives in `%LOCALAPPDATA%\Programs\Stellar\Data`:

- `settings.json` (plus `.bak`) is written atomically.
- `profiles\` holds the named profiles.
- `stellar.log` is the log.

Nothing is sent over the network.

## Layout

| Path | Purpose |
|---|---|
| `src/curves.rs` | curve kinds, validation, evaluation |
| `src/cooling.rs` | fan policy, safeguards, output ownership, tuning maths |
| `src/tuning.rs` | auto-tune sweep state machine and result builder |
| `src/engine.rs` | control loop thread |
| `src/openrgb.rs`, `src/lighting.rs` | native OpenRGB client and effects |
| `src/config.rs` | settings, profiles, migration |
| `src/ui.rs`, `src/ui/cooling_page.rs` | egui dashboard |
| `hwhost/` | LibreHardwareMonitor host (C#, ~150 lines) |

The sensor and fan-write layer reuses LibreHardwareMonitor through the small host process. It's the same driver stack the previous app ran on this PC (NCT6799D Super I/O and the RTX 5070), and it was already verified on this hardware. Rewriting Super I/O register access from scratch would put untested writes on live fans.
