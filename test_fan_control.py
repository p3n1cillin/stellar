import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from fan_control import FanControlBridge
from main import _atomic_write


class FanControlTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = Path(directory.name)
        self.bridge = FanControlBridge(self.directory / "settings")
        self.bridge.path = self.directory / "installed" / "FanControl.exe"
        self.source = {"__VERSION__": "275", "FanControl": {"Controls": [
            {"Identifier": "fan/0", "Enable": True, "ManualControl": False,
             "SelectedStart": 28, "Calibration": [[30, 604, False]]}],
            "FanCurves": [{"Name": "My curve", "Percent": 50}]}}

    def test_profile_copy_preserves_calibration_and_curves(self):
        original = copy.deepcopy(self.source)
        path = self.bridge.save_profile("My cooling", self.source, _atomic_write)
        self.assertEqual(self.bridge.read_profile(path), original)
        self.assertEqual(self.source, original)
        with self.assertRaises(ValueError):
            self.bridge.save_profile("My cooling", self.source, _atomic_write)

    def test_profile_name_cannot_escape_profile_directory(self):
        for name in ("../overwrite", "C:\\config", "CON", "name."):
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.bridge.save_profile(name, self.source, _atomic_write)

    def test_discovery_excludes_non_configs_and_backups(self):
        folder = self.bridge.path.parent / "Configurations"
        folder.mkdir(parents=True)
        (folder / "userConfig.json").write_text(json.dumps(self.source))
        (folder / "settings.json").write_text('{}')
        (folder / "Backups").mkdir()
        (folder / "Backups" / "old.json").write_text(json.dumps(self.source))
        self.assertEqual(self.bridge.profiles(), [folder / "userConfig.json"])

    def test_apply_uses_documented_config_argument_and_keeps_original(self):
        path = self.bridge.save_profile("My cooling", self.source, _atomic_write)
        before = path.read_bytes()
        shell = Mock(return_value=42)
        with patch("fan_control.ctypes.windll.shell32.ShellExecuteW", shell):
            self.bridge.apply(path)
        args = shell.call_args.args
        self.assertEqual(args[1], "runas")
        self.assertIn('-c "', args[3])
        self.assertIn(str(path.resolve()), args[3])
        self.assertEqual(path.read_bytes(), before)

    def test_existing_fan_control_is_not_restarted(self):
        with patch.object(self.bridge, "refresh", return_value=True), patch("fan_control.subprocess.run") as run:
            self.bridge.ensure_running()
        run.assert_not_called()

    def test_editor_overrides_fan_controls_minimized_setting(self):
        shell = Mock(return_value=42)
        with patch("fan_control.ctypes.windll.shell32.ShellExecuteW", shell):
            self.bridge.open_editor()
        self.assertEqual(shell.call_args.args[3], "-w")


if __name__ == "__main__":
    unittest.main()
