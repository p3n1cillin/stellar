# RGB Tray Controller

A small Windows system-tray app to turn your PC's RGB lighting on/off and set
its color, via [OpenRGB](https://openrgb.org) — free and open-source, works
across most RGB motherboards/RAM/GPUs/keyboards regardless of brand (no
vendor SDK, no account, no cloud).

## Setup

1. Install [OpenRGB](https://openrgb.org/) and launch it once so it detects
   your devices.
2. In OpenRGB: **Settings > SDK Server**, check **Server Enabled** (default
   port 6742). Leave OpenRGB running in the background — this app talks to
   OpenRGB, it doesn't replace it.
3. From this folder:
   ```
   python -m venv .venv
   .venv\Scripts\pip install -r requirements.txt
   ```
4. Run it:
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
- **Red** = on, but can't reach OpenRGB (check it's running with the SDK
  server enabled, then use **Reconnect to OpenRGB**)

Left-click (or the menu's top item) toggles on/off. Right-click for the full
menu: preset colors, **Custom Color...** (opens a color picker), reconnect,
and quit.

Your last color and on/off state are remembered across restarts, in a local
`config.json` next to `main.py` (not committed to git).

## Notes

- "Off" uses each device's own hardware Off mode when it has one; devices
  that don't expose one (most peripherals only have Direct/Static/Effect
  modes) are set to black instead — still fully dark, just not power-off.
- Everything runs locally against `127.0.0.1:6742`; nothing leaves your
  machine.
- Not code-signed — Windows SmartScreen may warn on first run of a script
  from an unfamiliar source. It's a plain Python script; read `main.py`
  yourself if you want to verify that before running it.
