using System.IO;

namespace CoolingLighting;

public static class SettingsTests
{
    public static void Run()
    {
        var directory = Path.Combine(Path.GetTempPath(), "CoolingLighting-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, "settings.json");
        var blockedBackup = Path.Combine(directory, "backup-failure.json.bak");
        var profilesDirectory = Path.Combine(directory, "profiles");
        var count = 0;
        void Check(bool condition, string name) { if (!condition) throw new Exception("FAIL: " + name); count++; }
        try
        {
            var saved = new NativeSettings { LightingOn = false, Color = [1, 2, 3],
                ZoneSizes = new() { ["Controller"] = new() { ["ARGB"] = 120 } },
                Fans = [new() { ControlId = "fan", Mode = "Curve", Name = "Case fans", RpmSensorId = "rpm", TemperatureSensorIds = ["cpu"] }] };
            SettingsStore.SaveTo(path, saved);
            var loaded = SettingsStore.LoadFrom(path);
            Check(!loaded.LightingOn && loaded.Color.SequenceEqual(saved.Color), "lighting round trip");
            Check(loaded.ZoneSizes["Controller"]["ARGB"] == 120, "LED count round trip");
            Check(loaded.Fans[0].TemperatureSensorIds[0] == "cpu" && loaded.Fans[0].RpmSensorId == "rpm", "fan rule round trip");
            Check(loaded.Fans[0].Points.SequenceEqual(saved.Fans[0].Points), "curve round trip");
            SettingsStore.AtomicWrite(path, "{bad json"u8.ToArray());
            Check(SettingsStore.LoadFrom(path).Fans.Count == 1, "corrupt primary recovers backup");
            SettingsStore.AtomicWrite(path, "{\"Fans\":[null]}"u8.ToArray());
            Check(SettingsStore.LoadFrom(path).Fans.Count == 1, "null rule recovers backup");
            SettingsStore.AtomicWrite(path, "{\"Fans\":[{\"ControlId\":\"fan\",\"Points\":null}]}"u8.ToArray());
            Check(SettingsStore.LoadFrom(path).Fans.Count == 1, "null curve recovers backup");
            SettingsStore.AtomicWrite(path, "{\"Version\":999}"u8.ToArray());
            Check(SettingsStore.LoadFrom(path).Fans.Count == 1, "unknown schema recovers backup");
            var invalid = saved with { Fans = [saved.Fans[0], saved.Fans[0]] };
            try { SettingsStore.SaveTo(path, invalid); throw new Exception("Duplicate fan accepted"); }
            catch (InvalidDataException) { count++; }
            Check(SettingsStore.LoadFrom(path).Fans.Count == 1, "failed validation preserves backup");
            var legacy = Path.Combine(directory, "legacy.json");
            var migrated = Path.Combine(directory, "migrated.json");
            var payload = "{\"on\":false,\"color\":[10,20,30],\"zone_sizes\":{\"Device\":{\"Channel\":24}}}"u8.ToArray();
            SettingsStore.AtomicWrite(legacy, payload);
            var imported = SettingsStore.LoadFrom(migrated, legacy);
            Check(imported.Color.SequenceEqual(new byte[] {10,20,30}) && !imported.LightingOn, "legacy lighting migration");
            Check(imported.ZoneSizes["Device"]["Channel"] == 24, "legacy LED count migration");
            Check(File.ReadAllBytes(legacy).SequenceEqual(payload), "legacy source remains untouched");
            Check(Directory.GetFiles(directory, "*.tmp").Length == 0, "atomic writes clean temporary files");
            SettingsStore.AtomicWrite(path + ".bak", "invalid backup"u8.ToArray());
            var corrupted = File.ReadAllBytes(path);
            Check(SettingsStore.LoadFrom(path).Fans.Count == 0 && SettingsStore.LastLoadWarning is not null, "corrupt copies fall back to firmware with warning");
            Check(File.ReadAllBytes(path).SequenceEqual(corrupted), "damaged settings preserved for recovery");
            var fixture = """
                {"FanControl":{"Controls":[
                  {"Identifier":"/lpc/nct6799d/control/0","NickName":"Case fan","IsHidden":false,"Enable":true,
                   "SelectedStart":28,"MinimumPercent":0,"ManualControlValue":50,"PairedFanSensor":{"Identifier":"/lpc/nct6799d/fan/0"},"Calibration":[[20,410,false],[100,1378,false]]},
                  {"Identifier":"NVApiWrapper/0/control/0","NickName":"GPU","IsHidden":false,"Enable":true,
                   "SelectedStart":9,"MinimumPercent":0,"ManualControlValue":50,"PairedFanSensor":{"Identifier":"old-rpm"},"Calibration":[]},
                  {"Identifier":"hidden","IsHidden":true,"Enable":false}
                ]}}
                """;
            var readings = new Dictionary<string, SensorReading> {
                ["/gpu-nvidia/0/control/1"] = new("/gpu-nvidia/0/control/1", "RTX", "Fan", "Control", 0, DateTimeOffset.UtcNow, true),
                ["/gpu-nvidia/0/fan/1"] = new("/gpu-nvidia/0/fan/1", "RTX", "Fan", "Fan", 0, DateTimeOffset.UtcNow, false)
            };
            var rules = FanControlImport.Parse(System.Text.Encoding.UTF8.GetBytes(fixture), readings);
            Check(rules.Count == 2 && rules.All(r => r.Mode == "Firmware"), "imports are metadata only, never activate fan writes");
            Check(rules[0].Calibration[0].Rpm == 410 && rules[0].StartPercent == 40 && rules[0].MinimumPercent == 30, "imports preserve calibration and conservative speed floors");
            Check(rules[1].ControlId == "/gpu-nvidia/0/control/1" && rules[1].RpmSensorId == "/gpu-nvidia/0/fan/1", "unambiguous NVIDIA backend mapping");
            readings["/gpu-nvidia/1/control/1"] = readings["/gpu-nvidia/0/control/1"] with {Id = "/gpu-nvidia/1/control/1", Hardware = "Other RTX"};
            Check(FanControlImport.Parse(System.Text.Encoding.UTF8.GetBytes(fixture), readings).Count == 1, "ambiguous GPUs never guessed");
            readings["/lpc/nct6799d/0/control/0"] = new("/lpc/nct6799d/0/control/0", "Board", "Fan #1", "Control", 50, DateTimeOffset.UtcNow, true);
            readings["/lpc/nct6799d/0/fan/0"] = new("/lpc/nct6799d/0/fan/0", "Board", "Fan #1", "Fan", 900, DateTimeOffset.UtcNow, false);
            var boardRule = FanControlImport.Parse(System.Text.Encoding.UTF8.GetBytes(fixture), readings)[0];
            Check(boardRule.ControlId == "/lpc/nct6799d/0/control/0" && boardRule.RpmSensorId == "/lpc/nct6799d/0/fan/0", "motherboard instance IDs map exact channels");
            readings["/lpc/nct6799d/1/control/0"] = readings["/lpc/nct6799d/0/control/0"] with {Id = "/lpc/nct6799d/1/control/0"};
            try { FanControlImport.Parse(System.Text.Encoding.UTF8.GetBytes(fixture), readings); throw new Exception("Ambiguous board mapped"); }
            catch (InvalidDataException) { count++; }
            Directory.CreateDirectory(blockedBackup);
            var backupFailurePath = Path.Combine(directory, "backup-failure.json");
            SettingsStore.SaveTo(backupFailurePath, saved);
            Check(SettingsStore.LastSaveWarning is not null, "backup failure reports a warning rather than false save failure");
            Check(SettingsStore.LoadFrom(backupFailurePath).Fans.Count == 1, "primary commit remains authoritative after backup failure");
            SettingsStore.SaveTo(path, saved);
            Check(SettingsStore.LastSaveWarning is null, "successful backup clears previous warning");
            ProfileStore.SaveIn(profilesDirectory, "Quiet", saved);
            ProfileStore.SaveIn(profilesDirectory, "Gaming", saved with {LightingOn = true});
            Check(ProfileStore.NamesIn(profilesDirectory).SequenceEqual(new[] {"Gaming", "Quiet"}), "named profiles enumerate once without backup copies");
            Check(ProfileStore.LoadFrom(profilesDirectory, "Quiet").ActiveProfile == "Quiet", "profile name survives disk reload");
            Check(!ProfileStore.LoadFrom(profilesDirectory, "Quiet").LightingOn && ProfileStore.LoadFrom(profilesDirectory, "Gaming").LightingOn, "named profiles stay independent");
            SettingsStore.AtomicWrite(Path.Combine(profilesDirectory, "Quiet.json"), "invalid profile"u8.ToArray());
            Check(ProfileStore.LoadFrom(profilesDirectory, "Quiet").Fans.Count == 1, "profile recovers corrupt primary from backup");
            SettingsStore.AtomicWrite(Path.Combine(profilesDirectory, "Quiet.json.bak"), "invalid profile backup"u8.ToArray());
            try { ProfileStore.LoadFrom(profilesDirectory, "Quiet"); throw new Exception("Corrupt profile silently activated"); }
            catch (InvalidDataException) { count++; }
            foreach (var badName in new[] {"../escape", "CON", "NUL", "COM1", "trailing ", "", new string('x', 61)})
            {
                try { ProfileStore.SaveIn(profilesDirectory, badName, saved); throw new Exception("Invalid profile name accepted"); }
                catch (ArgumentException) { count++; }
            }
            Check(!File.Exists(Path.Combine(directory, "escape.json")), "profile traversal creates no escaped file");
            Console.WriteLine($"PASS: {count} persistence checks");
        }
        finally
        {
            // Only files created in this unique test directory are removed.
            foreach (var file in Directory.GetFiles(directory)) File.Delete(file);
            if (Directory.Exists(blockedBackup)) Directory.Delete(blockedBackup);
            if (Directory.Exists(profilesDirectory))
            {
                foreach (var file in Directory.GetFiles(profilesDirectory)) File.Delete(file);
                Directory.Delete(profilesDirectory);
            }
            Directory.Delete(directory);
        }
    }
}
