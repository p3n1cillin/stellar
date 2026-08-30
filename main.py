"""RGB Tray Controller

A small Windows system-tray app that turns PC RGB lighting on/off and sets
its color, via OpenRGB (https://openrgb.org) — free, open-source, and works
across most RGB motherboards/RAM/GPUs/peripherals regardless of brand.

Setup:
    1. Install OpenRGB and launch it once.
    2. In OpenRGB: Settings > SDK Server > check "Server Enabled" (default
       port 6742). Leave OpenRGB running in the background.
    3. pip install -r requirements.txt
    4. python main.py   (or: pythonw main.py, to run without a console window)

The tray icon reflects current state: colored circle = on with that color,
gray = off, red = can't reach OpenRGB.
"""

import json
import sys
import threading
from pathlib import Path

from PIL import Image, ImageDraw
import pystray
from openrgb import OpenRGBClient
from openrgb.utils import RGBColor

CONFIG_PATH = Path(__file__).resolve().parent / "config.json"
DEFAULT_COLOR = (0, 120, 255)
DISCONNECTED_COLOR = (200, 40, 40)
OFF_COLOR = (60, 60, 60)
ICON_SIZE = 64

PRESET_COLORS = [
    ("Red", (255, 0, 0)),
    ("Orange", (255, 100, 0)),
    ("Yellow", (255, 220, 0)),
    ("Green", (0, 255, 0)),
    ("Cyan", (0, 220, 255)),
    ("Blue", (0, 80, 255)),
    ("Purple", (140, 0, 255)),
    ("Pink", (255, 0, 150)),
    ("White", (255, 255, 255)),
]


def load_config():
    """Returns (color_tuple, is_on). Falls back to defaults on any bad/missing file."""
    if CONFIG_PATH.exists():
        try:
            data = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
            color = tuple(int(c) for c in data.get("color", DEFAULT_COLOR))
            on = bool(data.get("on", True))
            if len(color) == 3 and all(0 <= c <= 255 for c in color):
                return color, on
        except (json.JSONDecodeError, OSError, ValueError, TypeError):
            pass
    return DEFAULT_COLOR, True


def save_config(color, on):
    try:
        CONFIG_PATH.write_text(
            json.dumps({"color": list(color), "on": on}), encoding="utf-8"
        )
    except OSError:
        pass


class RGBController:
    """Thin wrapper around the OpenRGB SDK client.

    Every public method is safe to call even when OpenRGB isn't running yet
    (returns False instead of raising) and will retry the connection on the
    next call rather than latching a permanent failure — OpenRGB can be
    started or restarted at any time relative to this app.
    """

    def __init__(self):
        self._client = None
        self._lock = threading.Lock()

    def is_connected(self):
        return self._client is not None

    def connect(self):
        with self._lock:
            if self._client is not None:
                return True
            try:
                self._client = OpenRGBClient(name="RGB Tray Controller")
                return True
            except Exception:
                self._client = None
                return False

    def disconnect(self):
        with self._lock:
            if self._client is None:
                return
            try:
                self._client.disconnect()
            except Exception:
                pass
            self._client = None

    def _live_devices(self):
        """Refreshed device list, or [] on any failure (also drops the stale connection)."""
        if self._client is None:
            return []
        try:
            self._client.update()
            return list(self._client.devices)
        except Exception:
            self.disconnect()
            return []

    def set_color(self, rgb):
        if not self.is_connected() and not self.connect():
            return False
        devices = self._live_devices()
        if not devices:
            return False
        color = RGBColor(*rgb)
        ok = True
        for device in devices:
            try:
                device.set_color(color)
            except Exception:
                ok = False
        return ok

    def turn_off(self):
        """Prefers each device's own "Off" mode when it has one (true hardware
        off, e.g. fully disabling a motherboard header) and falls back to a
        black Direct color otherwise (most peripherals only ever expose
        Direct/Static/Effect modes, never a real Off)."""
        if not self.is_connected() and not self.connect():
            return False
        devices = self._live_devices()
        if not devices:
            return False
        ok = True
        for device in devices:
            try:
                device.set_mode("Off")  # raises ValueError if this device has no Off mode
            except ValueError:
                try:
                    device.set_color(RGBColor(0, 0, 0))
                except Exception:
                    ok = False
            except Exception:
                ok = False
        return ok


def make_icon_image(rgb):
    image = Image.new("RGBA", (ICON_SIZE, ICON_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    pad = 4
    draw.ellipse(
        [pad, pad, ICON_SIZE - pad, ICON_SIZE - pad],
        fill=(*rgb, 255),
        outline=(20, 20, 20, 255),
        width=2,
    )
    return image


class TrayApp:
    def __init__(self):
        self.controller = RGBController()
        self.color, self.on = load_config()
        self.icon = pystray.Icon(
            "rgb-tray-app",
            make_icon_image(OFF_COLOR),
            "RGB Tray Controller — starting...",
            menu=self._build_menu(),
        )

    def _display_color(self):
        if not self.controller.is_connected() and self.on:
            # Only shown briefly: refreshed to the real state right after connect() runs.
            return DISCONNECTED_COLOR
        return self.color if self.on else OFF_COLOR

    def _refresh_icon(self):
        connected = self.controller.is_connected()
        color = self.color if self.on else OFF_COLOR
        if self.on and not connected:
            color = DISCONNECTED_COLOR
        self.icon.icon = make_icon_image(color)
        state = "on" if self.on else "off"
        conn = "connected" if connected else "OpenRGB not found"
        self.icon.title = f"RGB Tray Controller — {state} ({conn})"

    def _build_menu(self):
        color_items = tuple(
            pystray.MenuItem(name, self._make_color_handler(rgb))
            for name, rgb in PRESET_COLORS
        )
        return pystray.Menu(
            pystray.MenuItem(
                lambda item: "Turn Off" if self.on else "Turn On",
                self._toggle,
                default=True,
            ),
            pystray.MenuItem("Colors", pystray.Menu(*color_items)),
            pystray.MenuItem("Custom Color...", self._custom_color),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Reconnect to OpenRGB", self._reconnect),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Quit", self._quit),
        )

    def _make_color_handler(self, rgb):
        def handler(icon, item):
            self._apply_color(rgb)
        return handler

    def _apply_color(self, rgb):
        self.color = rgb
        self.on = True
        threading.Thread(target=self.controller.set_color, args=(rgb,), daemon=True).start()
        save_config(self.color, self.on)
        self._refresh_icon()

    def _toggle(self, icon, item):
        if self.on:
            self.on = False
            threading.Thread(target=self.controller.turn_off, daemon=True).start()
        else:
            self.on = True
            threading.Thread(target=self.controller.set_color, args=(self.color,), daemon=True).start()
        save_config(self.color, self.on)
        self._refresh_icon()

    def _custom_color(self, icon, item):
        # tkinter's color chooser must run on its own thread with its own Tk
        # root — pystray's menu callback thread is not a GUI thread and must
        # not be blocked waiting on it, or the tray stops responding.
        def pick():
            import tkinter as tk
            from tkinter import colorchooser

            root = tk.Tk()
            root.withdraw()
            result = colorchooser.askcolor(
                color="#%02x%02x%02x" % self.color, title="Choose RGB color"
            )
            root.destroy()
            rgb = result[0]
            if rgb is not None:
                self._apply_color(tuple(int(c) for c in rgb))

        threading.Thread(target=pick, daemon=True).start()

    def _reconnect(self, icon, item):
        def do_reconnect():
            self.controller.disconnect()
            self.controller.connect()
            self._refresh_icon()
            if self.on:
                self.controller.set_color(self.color)
        threading.Thread(target=do_reconnect, daemon=True).start()

    def _quit(self, icon, item):
        self.icon.stop()

    def _startup(self, icon):
        icon.visible = True
        self.controller.connect()
        self._refresh_icon()
        if self.on:
            threading.Thread(target=self.controller.set_color, args=(self.color,), daemon=True).start()
        else:
            threading.Thread(target=self.controller.turn_off, daemon=True).start()

    def run(self):
        self.icon.run(setup=self._startup)


def main():
    if sys.platform != "win32":
        print("This tray app targets Windows; other platforms are untested.", file=sys.stderr)
    TrayApp().run()


if __name__ == "__main__":
    main()
