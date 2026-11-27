"""Integration through Fan Control's documented configuration command line.

Fan Control owns hardware access and cooling. This module never writes PWM,
changes an installed configuration, or stops the cooling process.
"""

import ctypes
import csv
import io
import json
import os
from pathlib import Path
import subprocess


class FanControlBridge:
    def __init__(self, settings_dir):
        self.settings_dir = Path(settings_dir)
        self.profile_dir = self.settings_dir / "FanProfiles"
        self.path = self.find_executable()
        self.running = False
        self.error = ""
        self.pids = set()

    @staticmethod
    def find_executable():
        candidates = [os.environ.get("FANCONTROL_PATH")]
        for base in (os.environ.get("ProgramFiles(x86)"), os.environ.get("ProgramFiles")):
            if base:
                candidates.append(str(Path(base) / "FanControl" / "FanControl.exe"))
        candidates.append(str(Path("C:/FanControl/FanControl.exe")))
        return next((Path(p) for p in candidates if p and Path(p).is_file()), None)

    def refresh(self):
        try:
            result = subprocess.run(
                ["tasklist", "/FI", "IMAGENAME eq FanControl.exe", "/FO", "CSV", "/NH"],
                capture_output=True, text=True, timeout=5,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            rows = csv.reader(io.StringIO(result.stdout))
            self.pids = {int(row[1]) for row in rows
                         if len(row) > 1 and row[0].lower() == "fancontrol.exe" and row[1].isdigit()}
            self.running = bool(self.pids)
            self.error = "" if self.path else "Fan Control installation was not found"
        except (OSError, subprocess.TimeoutExpired) as exc:
            self.error = str(exc)
        return self.running

    def profiles(self):
        """Only user configs; internal CACHE and version backups are excluded."""
        paths = []
        if self.path:
            config_dir = self.path.parent / "Configurations"
            try:
                cache = json.loads((config_dir / "CACHE").read_text(encoding="utf-8-sig"))
                custom = cache.get("CustomConfigFolder")
                if custom and Path(custom).is_dir():
                    config_dir = Path(custom)
            except (OSError, ValueError, AttributeError):
                pass
            paths.extend(config_dir.glob("*.json"))
        paths.extend(self.profile_dir.glob("*.json"))
        valid = []
        for path in sorted(set(paths), key=lambda p: (p.stem.lower(), str(p))):
            try:
                self.read_profile(path)
                valid.append(path)
            except (OSError, ValueError, TypeError):
                continue
        return valid

    @staticmethod
    def read_profile(path):
        data = json.loads(Path(path).read_text(encoding="utf-8-sig"))
        if not isinstance(data, dict) or not isinstance(data.get("FanControl"), dict):
            raise ValueError("This file is not a Fan Control profile")
        controls = data["FanControl"].get("Controls")
        if not isinstance(controls, list) or any(not isinstance(c, dict) for c in controls):
            raise ValueError("Fan Control profile has invalid controls")
        return data

    def save_profile(self, name, data, atomic_write):
        name = name.strip()
        if (not name or len(name) > 60 or any(c in name for c in '<>:"/\\|?*')
                or name.endswith((".", " "))
                or name.split(".")[0].upper() in {"CON", "PRN", "AUX", "NUL", *[f"COM{i}" for i in range(1, 10)], *[f"LPT{i}" for i in range(1, 10)]}):
            raise ValueError("Use a short profile name without special characters")
        target = self.profile_dir / (name + ".json")
        if target.exists():
            raise ValueError("A profile with that name already exists")
        atomic_write(target, json.dumps(data, indent=2))
        return target

    def apply(self, profile):
        profile = Path(profile).resolve()
        self.read_profile(profile)
        if not self.path:
            raise RuntimeError("Fan Control is not installed")
        # Existing Fan Control runs elevated. ShellExecute uses Windows' normal
        # elevation prompt, allowing its supported IPC command to reach it.
        result = ctypes.windll.shell32.ShellExecuteW(
            None, "runas", str(self.path), subprocess.list2cmdline(["-c", str(profile)]),
            str(self.path.parent), 0,
        )
        if result <= 32:
            raise RuntimeError("Windows did not launch the Fan Control profile command")

    def ensure_running(self):
        if self.refresh():
            return
        # Reuse the user's existing elevated startup task, without changing it.
        result = subprocess.run(["schtasks", "/Run", "/TN", "FanControl"],
                                capture_output=True, text=True, timeout=10,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or "Could not start the FanControl task")

    def open_editor(self):
        if not self.path:
            raise RuntimeError("Fan Control is not installed")
        # Documented --window flag overrides Fan Control's StartMinimized setting.
        result = ctypes.windll.shell32.ShellExecuteW(
            None, "runas", str(self.path), "-w", str(self.path.parent), 1)
        if result <= 32:
            raise RuntimeError("Windows did not open Fan Control")

    @staticmethod
    def control_summary(control):
        if not control.get("Enable"):
            return "Disabled"
        if control.get("ManualControl"):
            return f"Manual: {control.get('ManualControlValue', '?')}%"
        curve = control.get("SelectedFanCurve")
        if isinstance(curve, dict):
            curve = curve.get("Name", "Assigned curve")
        return f"Curve: {curve}" if curve else "No curve assigned in saved profile"
