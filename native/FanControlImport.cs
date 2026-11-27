using System.IO;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace CoolingLighting;

public static class FanControlImport
{
    private static string MapMotherboardId(string oldId, IReadOnlyDictionary<string, SensorReading> readings, bool output)
    {
        if (readings.TryGetValue(oldId, out var exact) && (output ? exact.CanControl : exact.Type == "Fan")) return oldId;
        // LHM now includes the chip instance. Match the exact chip/type/channel,
        // never a display name or channel number from a different controller.
        if (!Regex.IsMatch(oldId, @"^/lpc/[^/]+/(control|fan)/\d+$")) return oldId;
        var candidates = readings.Values.Where(s => (output ? s.CanControl : s.Type == "Fan") &&
            Regex.Replace(s.Id, @"^(/lpc/[^/]+)/\d+/(control|fan)/(\d+)$", "$1/$2/$3") == oldId).ToArray();
        if (candidates.Length > 1) throw new InvalidDataException("Ambiguous motherboard channel: " + oldId);
        return candidates.Length == 1 ? candidates[0].Id : oldId;
    }
    public static string DefaultPath => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "FanControl", "Configurations", "userConfig.json");

    public static List<FanRule> Parse(byte[] payload, IReadOnlyDictionary<string, SensorReading> readings)
    {
        using var document = JsonDocument.Parse(payload);
        var rules = new List<FanRule>();
        foreach (var item in document.RootElement.GetProperty("FanControl").GetProperty("Controls").EnumerateArray())
        {
            if (!item.GetProperty("Enable").GetBoolean() || item.GetProperty("IsHidden").GetBoolean()) continue;
            var oldId = item.GetProperty("Identifier").GetString() ?? "";
            var id = oldId;
            var rpm = item.GetProperty("PairedFanSensor").GetProperty("Identifier").GetString();
            if (oldId.StartsWith("/lpc/", StringComparison.Ordinal))
            {
                id = MapMotherboardId(oldId, readings, true);
                if (rpm is not null) rpm = MapMotherboardId(rpm, readings, false);
            }
            if (oldId.StartsWith("NVApiWrapper/", StringComparison.Ordinal))
            {
                // Backends use different identifiers. Only map unambiguous detected hardware.
                var outputs = readings.Values.Where(s => s.CanControl && s.Id.StartsWith("/gpu-nvidia/", StringComparison.Ordinal)).ToArray();
                if (outputs.Length != 1) continue;
                id = outputs[0].Id;
                var fans = readings.Values.Where(s => s.Type == "Fan" && s.Hardware == outputs[0].Hardware).ToArray();
                rpm = fans.Length == 1 ? fans[0].Id : null;
            }
            var calibration = item.TryGetProperty("Calibration", out var points) && points.ValueKind == JsonValueKind.Array
                ? points.EnumerateArray().Select(p => new CalibrationPoint(p[0].GetDouble(), p[1].GetDouble())).ToArray() : [];
            var rule = new FanRule
            {
                ControlId = id, Name = item.GetProperty("NickName").GetString() ?? id,
                RpmSensorId = rpm, Calibration = calibration,
                // Import metadata only. A saved legacy curve need not reflect live unsaved settings.
                Mode = "Firmware",
                StartPercent = Math.Max(40, item.GetProperty("SelectedStart").GetDouble()),
                MinimumPercent = Math.Max(30, item.GetProperty("MinimumPercent").GetDouble()),
                ManualPercent = item.GetProperty("ManualControlValue").GetDouble()
            };
            rule.Validate();
            rules.Add(rule);
        }
        if (rules.Select(r => r.ControlId).Distinct().Count() != rules.Count) throw new InvalidDataException("Duplicate imported outputs");
        return rules;
    }

    public static List<FanRule> ReadAndBackUp(IReadOnlyDictionary<string, SensorReading> readings)
    {
        var payload = File.ReadAllBytes(DefaultPath);
        var rules = Parse(payload, readings);
        var directory = Path.Combine(SettingsStore.DirectoryPath, "imports");
        Directory.CreateDirectory(directory);
        SettingsStore.AtomicWrite(Path.Combine(directory, "FanControl-" + DateTime.UtcNow.ToString("yyyyMMdd-HHmmss-fff") + ".json"), payload);
        return rules;
    }
}
