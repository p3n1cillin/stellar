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
import logging
import os
import shutil
import subprocess
import sys
import threading
import time
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw
import pystray
from openrgb import OpenRGBClient
from openrgb.utils import RGBColor

LEGACY_CONFIG_PATH = Path(__file__).resolve().parent / "config.json"
SETTINGS_DIR = Path(os.environ.get("LOCALAPPDATA", str(LEGACY_CONFIG_PATH.parent))) / "RGBTrayController"
CONFIG_PATH = SETTINGS_DIR / "config.json"
CONFIG_LOCK = threading.RLock()
DEFAULT_COLOR = (0, 120, 255)
DISCONNECTED_COLOR = (200, 40, 40)
OFF_COLOR = (60, 60, 60)
ICON_SIZE = 64
OPENRGB_STARTUP_TIMEOUT = 15.0

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
    """Returns (color_tuple, is_on, zone_sizes), with safe defaults."""
    paths = (CONFIG_PATH, CONFIG_PATH.with_suffix(".json.bak"), LEGACY_CONFIG_PATH)
    for path in paths:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(data, dict):
                raise ValueError("Settings must be an object")
            color = data.get("color", DEFAULT_COLOR)
            on = data.get("on", True)
            if (not isinstance(color, (list, tuple)) or len(color) != 3
                    or any(type(c) is not int or not 0 <= c <= 255 for c in color)
                    or type(on) is not bool):
                raise ValueError("Invalid color or on/off state")
            zone_sizes = data.get("zone_sizes", {})
            if not isinstance(zone_sizes, dict):
                zone_sizes = {}
            zone_sizes = {
                device: {zone: size for zone, size in zones.items()
                         if isinstance(zone, str) and type(size) is int and 0 <= size <= 10000}
                for device, zones in zone_sizes.items()
                if isinstance(device, str) and isinstance(zones, dict)
            }
            if path != CONFIG_PATH:
                save_config(tuple(color), on, zone_sizes)
            return tuple(color), on, zone_sizes
        except (json.JSONDecodeError, OSError, ValueError, TypeError):
            continue
    return DEFAULT_COLOR, True, {}


def _atomic_write(path, payload):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent,
                                         prefix=path.name, suffix=".tmp", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def save_config(color, on, zone_sizes):
    payload = json.dumps({"color": list(color), "on": on, "zone_sizes": zone_sizes}, indent=2)
    try:
        with CONFIG_LOCK:
            _atomic_write(CONFIG_PATH, payload)
            _atomic_write(CONFIG_PATH.with_suffix(".json.bak"), payload)
        return True
    except OSError:
        logging.exception("Could not save RGB settings")
        return False


class RGBController:
    """Thin wrapper around the OpenRGB SDK client.

    Every public method is safe to call even when OpenRGB isn't running yet
    (returns False instead of raising) and will retry the connection on the
    next call rather than latching a permanent failure — OpenRGB can be
    started or restarted at any time relative to this app.
    """

    def __init__(self, zone_sizes=None):
        self._client = None
        self._lock = threading.RLock()
        self._server_process = None
        self._server_start_attempted = False
        self._zone_sizes = zone_sizes or {}

    def is_connected(self):
        with self._lock:
            return self._client is not None

    @staticmethod
    def _find_openrgb():
        """Find an OpenRGB executable installed on Windows."""
        configured = os.environ.get("OPENRGB_PATH")
        candidates = [configured, shutil.which("OpenRGB")]
        for env_name in ("ProgramFiles", "ProgramFiles(x86)"):
            base = os.environ.get(env_name)
            if base:
                candidates.append(str(Path(base) / "OpenRGB" / "OpenRGB.exe"))
        for candidate in candidates:
            if candidate and Path(candidate).is_file():
                return candidate
        return None

    def _try_connect(self):
        try:
            self._client = OpenRGBClient(name="RGB Tray Controller")
            return True
        except Exception:
            self._client = None
            return False

    def _start_openrgb_server(self):
        """Start a local SDK server when OpenRGB is installed but not enabled."""
        if sys.platform != "win32":
            return False
        if self._server_process is not None and self._server_process.poll() is None:
            return False
        self._server_start_attempted = True
        executable = self._find_openrgb()
        if executable is None:
            return False

        startupinfo = subprocess.STARTUPINFO()
        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startupinfo.wShowWindow = subprocess.SW_HIDE
        try:
            self._server_process = subprocess.Popen(
                [executable, "--server", "--server-host", "127.0.0.1"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                startupinfo=startupinfo,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            return True
        except OSError:
            self._server_process = None
            return False

    def connect(self):
        with self._lock:
            if self._client is not None:
                return True
            if self._try_connect():
                return True
            if not self._start_openrgb_server():
                return False
            deadline = time.monotonic() + OPENRGB_STARTUP_TIMEOUT
            while time.monotonic() < deadline:
                time.sleep(0.25)
                if self._try_connect():
                    return True
                if self._server_process.poll() is not None:
                    break
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

    def close(self):
        """Release the SDK client and a server process started by this app."""
        self.disconnect()
        process = self._server_process
        if process is not None and process.poll() is None:
            try:
                process.terminate()
                process.wait(timeout=3)
            except (OSError, subprocess.TimeoutExpired):
                pass
        self._server_process = None

    def _live_devices(self):
        """Refreshed device list, or [] on any failure (also drops the stale connection)."""
        if self._client is None:
            return []
        try:
            self._client.update()
            devices = list(self._client.devices)
            resized = False
            for device in devices:
                configured_zones = self._zone_sizes.get(device.name, {})
                if not isinstance(configured_zones, dict):
                    continue
                for zone in device.zones:
                    size = configured_zones.get(zone.name)
                    if isinstance(size, int) and size >= 0 and len(zone.leds) != size:
                        zone.resize(size)
                        resized = True
            if resized:
                self._client.update()
                devices = list(self._client.devices)
            return devices
        except Exception:
            try:
                self._client.disconnect()
            except Exception:
                pass
            self._client = None
            return []

    def set_color(self, rgb):
        with self._lock:
            if not self.connect():
                return False
            devices = self._live_devices()
            if not devices:
                return False
            color = RGBColor(*rgb)
            ok = True
            for device in devices:
                try:
                    # A device in its hardware Off mode ignores color updates.
                    # Custom mode selects its most granular controllable mode
                    # (normally Direct), restoring it before applying the color.
                    device.set_custom_mode()
                    device.set_color(color)
                except Exception:
                    ok = False
            return ok

    def turn_off(self):
        """Prefers each device's own "Off" mode when it has one (true hardware
        off, e.g. fully disabling a motherboard header) and falls back to a
        black Direct color otherwise (most peripherals only ever expose
        Direct/Static/Effect modes, never a real Off)."""
        with self._lock:
            if not self.connect():
                return False
            devices = self._live_devices()
            if not devices:
                return False
            ok = True
            for device in devices:
                try:
                    device.set_mode("Off")  # raises ValueError if no Off mode exists
                except ValueError:
                    try:
                        device.set_custom_mode()
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
        self.color, self.on, self.zone_sizes = load_config()
        self.controller = RGBController(self.zone_sizes)
        self._state_lock = threading.RLock()
        self._changed = threading.Event()
        self._stopping = threading.Event()
        self._applied = False
        self._save_ok = True
        self._worker = None
        self.dashboard = None
        self.icon = pystray.Icon(
            "rgb-tray-app",
            make_icon_image(OFF_COLOR),
            "Cooling & Lighting — starting...",
            menu=self._build_menu(),
        )

    def _refresh_icon(self):
        color = self.color if self.on else OFF_COLOR
        if not self._applied or not self._save_ok:
            color = DISCONNECTED_COLOR
        self.icon.icon = make_icon_image(color)
        state = "on" if self.on else "off"
        conn = "applied" if self._applied else "waiting for OpenRGB"
        if not self._save_ok:
            conn = "settings could not be saved"
        self.icon.title = f"Cooling & Lighting — {state} ({conn})"
        self.icon.update_menu()

    def _build_menu(self):
        color_items = tuple(
            pystray.MenuItem(name, self._make_color_handler(rgb))
            for name, rgb in PRESET_COLORS
        )
        return pystray.Menu(
            pystray.MenuItem("Open Cooling & Lighting", self._open_dashboard, default=True),
            pystray.MenuItem(
                lambda item: "Turn Off" if self.on else "Turn On",
                self._toggle,
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

    def _save_and_schedule(self):
        self._save_ok = save_config(self.color, self.on, self.zone_sizes)
        self._applied = False
        self._changed.set()
        self._refresh_icon()

    def _work(self):
        """One worker applies the latest selection and recovers after SDK restarts."""
        while not self._stopping.is_set():
            requested = self._changed.wait(timeout=5)
            if self._stopping.is_set():
                break
            self._changed.clear()
            if self.dashboard:
                self.dashboard.monitor()
            with self._state_lock:
                color, on = self.color, self.on
            try:
                if not requested and self._applied:
                    with self.controller._lock:
                        healthy = bool(self.controller._live_devices())
                    if healthy:
                        continue
                success = (self.controller.set_color(color) if on
                           else self.controller.turn_off())
                if not success:
                    self.controller.disconnect()
                with self._state_lock:
                    self._applied = success and (color, on) == (self.color, self.on)
            except Exception:
                logging.exception("Could not apply RGB state")
                self._applied = False
                self.controller.disconnect()
            if not self._stopping.is_set():
                self._refresh_icon()

    def _apply_color(self, rgb):
        with self._state_lock:
            self.color = rgb
            self.on = True
            self._save_and_schedule()

    def _toggle(self, icon, item):
        with self._state_lock:
            self.on = not self.on
            self._save_and_schedule()

    def _custom_color(self, icon, item):
        if self.dashboard:
            self.dashboard.commands.put(("color", None))

    def _open_dashboard(self, icon, item):
        if self.dashboard:
            self.dashboard.commands.put(("show", None))

    def _reconnect(self, icon, item):
        self._applied = False
        self._changed.set()

    def _quit(self, icon, item):
        self._stopping.set()
        self._changed.set()
        self.icon.stop()
        if self.dashboard:
            self.dashboard.commands.put(("quit", None))

    def _startup(self, icon):
        icon.visible = True
        self._changed.set()
        self._worker = threading.Thread(target=self._work, daemon=True)
        self._worker.start()

    def run(self):
        from dashboard import Dashboard
        self.dashboard = Dashboard(self, SETTINGS_DIR, _atomic_write)
        tray = threading.Thread(target=self.icon.run, kwargs={"setup": self._startup}, daemon=True)
        tray.start()
        try:
            if "--show" in sys.argv:
                self.dashboard.show()
            self.dashboard.run()
        finally:
            self._stopping.set()
            self._changed.set()
            self.icon.stop()
            if self._worker:
                self._worker.join(timeout=20)
            self.controller.close()


def main():
    if sys.platform != "win32":
        print("This tray app targets Windows; other platforms are untested.", file=sys.stderr)
    SETTINGS_DIR.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(filename=SETTINGS_DIR / "app.log", level=logging.WARNING,
                        format="%(asctime)s %(levelname)s %(message)s")
    # Windows releases this lock even on shutdown or abrupt process termination.
    with (SETTINGS_DIR / "app.lock").open("a+b") as lock_file:
        if sys.platform == "win32":
            import msvcrt
            if os.fstat(lock_file.fileno()).st_size == 0:
                lock_file.write(b"0")
                lock_file.flush()
            lock_file.seek(0)
            try:
                msvcrt.locking(lock_file.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                if "--show" in sys.argv:
                    _atomic_write(SETTINGS_DIR / "show.request", "show")
                return
        TrayApp().run()


if __name__ == "__main__":
    main()
