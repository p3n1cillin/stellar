import unittest
import tempfile
from pathlib import Path
from unittest.mock import Mock, patch

from main import RGBController, load_config, save_config


class SettingsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "settings" / "config.json"
        self.legacy = Path(self.directory.name) / "legacy.json"
        config_patch = patch("main.CONFIG_PATH", self.path)
        legacy_patch = patch("main.LEGACY_CONFIG_PATH", self.legacy)
        config_patch.start()
        legacy_patch.start()
        self.addCleanup(config_patch.stop)
        self.addCleanup(legacy_patch.stop)

    def test_color_off_and_fan_layout_survive_reload(self):
        zones = {"Corsair Lighting Node Core": {"Corsair Channel 1": 24}}
        self.assertTrue(save_config((12, 34, 56), False, zones))
        self.assertEqual(load_config(), ((12, 34, 56), False, zones))

    def test_interrupted_replace_keeps_previous_settings(self):
        save_config((12, 34, 56), False, {})
        with patch("main.os.replace", side_effect=OSError("simulated interruption")):
            with self.assertLogs(level="ERROR"):
                self.assertFalse(save_config((255, 0, 0), True, {}))
        self.assertEqual(load_config(), ((12, 34, 56), False, {}))
        self.assertEqual(list(self.path.parent.glob("*.tmp")), [])

    def test_damaged_primary_recovers_latest_backup(self):
        save_config((12, 34, 56), False, {})
        self.path.write_text('{"color":', encoding="utf-8")
        self.assertEqual(load_config(), ((12, 34, 56), False, {}))

    def test_old_project_settings_are_migrated(self):
        self.legacy.write_text('{"color": [1, 2, 3], "on": false, "zone_sizes": {"fans": {"channel": 24}}}', encoding="utf-8")
        expected = ((1, 2, 3), False, {"fans": {"channel": 24}})
        self.assertEqual(load_config(), expected)
        self.legacy.unlink()
        self.assertEqual(load_config(), expected)


class RGBControllerTests(unittest.TestCase):
    def make_controller(self, devices):
        controller = RGBController()
        for device in devices:
            device.zones = []
        controller._client = Mock()
        controller._client.devices = devices
        return controller

    def test_set_color_restores_custom_mode_before_setting_color(self):
        device = Mock()
        controller = self.make_controller([device])

        self.assertTrue(controller.set_color((12, 34, 56)))

        device.set_custom_mode.assert_called_once_with()
        device.set_color.assert_called_once()
        self.assertLess(
            device.method_calls.index(unittest.mock.call.set_custom_mode()),
            device.method_calls.index(unittest.mock.call.set_color(unittest.mock.ANY)),
        )

    def test_turn_off_falls_back_to_black_in_custom_mode(self):
        device = Mock()
        device.set_mode.side_effect = ValueError("no Off mode")
        controller = self.make_controller([device])

        self.assertTrue(controller.turn_off())

        device.set_custom_mode.assert_called_once_with()
        device.set_color.assert_called_once()

    def test_set_color_reports_a_device_failure(self):
        good_device = Mock()
        bad_device = Mock()
        bad_device.set_custom_mode.side_effect = RuntimeError("device lost")
        controller = self.make_controller([good_device, bad_device])

        self.assertFalse(controller.set_color((1, 2, 3)))
        good_device.set_color.assert_called_once()

    def test_configured_zone_size_is_restored(self):
        zone = Mock(name="zone")
        zone.name = "Corsair Channel 1"
        zone.leds = []
        device = Mock(name="device")
        device.name = "Corsair Lighting Node Core"
        device.zones = [zone]
        controller = RGBController(
            {"Corsair Lighting Node Core": {"Corsair Channel 1": 24}}
        )
        controller._client = Mock()
        controller._client.devices = [device]

        controller._live_devices()

        zone.resize.assert_called_once_with(24)


if __name__ == "__main__":
    unittest.main()
