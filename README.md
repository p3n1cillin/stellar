<p align="center"><img src="assets/stellar.png" width="96" alt=""></p>

<h1 align="center">Stellar</h1>
<p align="center"><b>All your PC's fans and RGB lighting in one place, for Windows.</b><br>
Fan curves like Fan Control, synced effects like the RGB suites, plus auto-tuning, without vendor bloat.</p>

---

## Features

- **Every fan you have.** Motherboard fan headers (including fan hubs) and graphics-card fans, through [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor).
- **Fan Control-style curves.** Several fans can share one curve. Curve types:
  - **Graph:** draggable points over one or more sensors.
  - **Linear** and **Flat**.
  - **Trigger:** switches between idle and load speeds, with hysteresis.
  - **Mix:** for example, the hotter of the CPU and GPU curves.
  - Every curve can have a **response time** (smoothing).

  Each fan also has start and stop speeds, step-up and step-down rates, an offset and hysteresis.
- **Auto-tune.** Click once and Stellar sweeps each header, learns where your fans stall and restart, and finds the point where extra speed adds little airflow. It then builds quiet CPU, GPU and case curves. It handles fan hubs, and has an AIO liquid-cooler mode.
- **Safety first.** Fans go to 100% instantly if a sensor disappears or goes stale, a fan stalls, or a temperature limit is hit. If Stellar ever crashes or hangs, all fans return to your motherboard's own control within 10 seconds.
- **RGB from every brand.** Motherboard headers, RAM, GPUs, Corsair/NZXT controllers, keyboards: anything [OpenRGB](https://openrgb.org) supports.
  - Effects: static, breathing, spectrum, rainbow wave, gradient, and **temperature-reactive** colour.
  - Per-device effects, brightness, and quiet hours.
- **Profiles and automation.** Save complete setups and switch automatically while a program runs (for example, a game).
- **Lightweight.** A small native app that lives in the tray, starts with Windows, and sends no telemetry.

## Install

1. Download **`Stellar-x.y.z-win-x64.zip`** from [Releases](../../releases/latest) and extract it anywhere.
2. Run **`stellar.exe`** and choose **Yes** to install. Approve the Windows administrator prompt once.
   - Stellar installs for your account in `%LOCALAPPDATA%\Programs\Stellar`.
   - It adds Start menu and desktop shortcuts and appears in **Settings › Apps**.
   - It starts elevated at sign-in through its own scheduled task, so there's **no prompt at each startup**.
3. If the **PawnIO** driver is missing, Stellar offers to install it. PawnIO is the free, signed driver LibreHardwareMonitor uses to reach motherboard fans and CPU temperatures.
4. Open **Cooling** and click **Auto-tune**. Untick any header with a **pump** on it; headers that look like pumps are unticked for you.
5. Optional: open **Lighting**, choose **Let Stellar control lighting**, and let Stellar install OpenRGB if needed.

To try Stellar without installing it, choose **No** at the install prompt (or run `stellar.exe --portable`).

> **"Windows protected your PC"?** Stellar isn't code-signed yet. Click **More info › Run anyway**. Some antivirus tools flag every hardware monitor because they load a kernel driver. Stellar uses PawnIO, the signed driver LibreHardwareMonitor itself uses. All source code is in this repository.

### Requirements

- Windows 10 or 11, 64-bit.
- An administrator account. Fan headers and CPU sensors need elevation; Stellar asks once, at install.
- For motherboard fans: a board whose Super I/O chip LibreHardwareMonitor supports (most Nuvoton, ITE and Fintek chips on desktop boards). Graphics-card fans work on NVIDIA and AMD.
- For lighting: OpenRGB. Stellar installs it on request.

## Using it

- **Overview:** temperatures with 10-minute graphs, every fan's speed and what controls it, and recent activity.
- **Cooling:**
  - **Fan cards:** each fan can be on firmware control, follow a curve, or run at a manual speed.
  - **Curves:** create and edit them in the library below the fan cards.
  - **Auto-tune / Re-run auto-tune:** measures your fans.
  - **Rebuild curves:** re-applies tuning without a new sweep, for example after switching **CPU cooler** between Air and AIO.
- **Lighting:** global effect, per-device effects, LED counts for addressable headers, and quiet hours.
- **Profiles & automation:** save and apply complete setups, or switch automatically while a program runs.
- **Tray:** click to open, right-click for lighting on/off, profiles and Quit. The icon turns red while a safety override is active.

Closing the window keeps Stellar running in the tray, so your fan curves stay active. **Quit Stellar** returns every fan to firmware control.

### Other fan and RGB software

Stellar won't fight other programs.

- **Fan Control, Argus Monitor or SpeedFan running:** Stellar leaves all fans on firmware control until they exit.
- **MSI Afterburner running:** Stellar leaves the graphics card's fans to Afterburner and keeps controlling everything else.
- **Other RGB apps** (Armoury Crate, iCUE, SignalRGB, …): close them before letting Stellar control lighting.

### Command line

```
stellar              open the dashboard (or the running instance)
stellar --tray       start in the tray
stellar --quit       return fans to firmware control and exit
stellar --autotune   tune every non-pump fan (running instance)
stellar --install    install or update from this folder
stellar --uninstall  remove Stellar (settings are kept)
stellar --portable   run without installing
stellar status       one-off hardware reading as JSON
stellar rgb          list OpenRGB devices
```

## Uninstall

Use **Settings › Apps › Stellar › Uninstall**, or run `stellar --uninstall`. Fans return to firmware control. Your settings stay in `%LOCALAPPDATA%\Programs\Stellar\Data`; delete that folder to remove them too.

## How it works

| Path | Purpose |
|---|---|
| `src/engine.rs` | control loop thread: reads sensors every second, applies fan policy, renders lighting |
| `src/cooling.rs` | fan policy and safeguards, output ownership, tuning maths |
| `src/curves.rs` | curve types, validation, evaluation and smoothing |
| `src/tuning.rs` | auto-tune sweep and curve builder |
| `src/openrgb.rs`, `src/lighting.rs` | native OpenRGB SDK client and effects engine |
| `src/config.rs` | settings, profiles, atomic saves and migration |
| `src/install.rs` | self-installer, sign-in task, Apps & Features entry |
| `src/ui.rs`, `src/ui/` | dashboard (egui) and tray |
| `hwhost/` | `stellar-hw`, a small LibreHardwareMonitor host (C#) |

Sensor reads and fan writes go through `stellar-hw`, a small LibreHardwareMonitor host process. It returns every fan it touched to firmware control when Stellar exits or crashes, or when Stellar stops responding for 10 seconds.

Settings are saved atomically with a backup copy. If both copies are damaged, fans stay on firmware control rather than guessing.

### Build from source

Requires Rust (stable, MSVC toolchain), the .NET 8 SDK and Windows.

```powershell
.\build.ps1          # tests, release build, self-contained hardware host -> .\dist\Stellar + zip
```

Releases are built by GitHub Actions when a `v*` tag is pushed (`.github/workflows/release.yml`).

## License

MIT; see [LICENSE](LICENSE). Bundled components are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

The `legacy/` folder holds Stellar's predecessors: a Python RGB tray app and a C# WPF app.
