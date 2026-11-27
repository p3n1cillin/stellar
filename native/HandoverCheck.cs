using System.IO;
using System.Text.Json;

namespace CoolingLighting;

// Explicit, attended cutover check. Never reached by ordinary startup.
public static class HandoverCheck
{
    public static void Run()
    {
        var reportPath = Path.Combine(SettingsStore.DirectoryPath, "handover-check.json");
        var samples = new List<object>();
        bool passed = false;
        string? error = null;
        using var mutex = new Mutex(false, "Local\\CoolingLighting.Native");
        bool acquired;
        try { acquired = mutex.WaitOne(0); } catch (AbandonedMutexException) { acquired = true; }
        if (!acquired) throw new InvalidOperationException("Quit the native app before testing");
        try
        {
            using var engine = new HardwareEngine();
            try
            {
                if (!engine.Administrator || HardwareEngine.FanControlRunning())
                    throw new InvalidOperationException("Administrator access and an exited Fan Control are required");
                var saved = SettingsStore.Load();
                if (SettingsStore.LastLoadWarning is not null || saved.Fans.Count == 0 || saved.Fans.Any(r => r.Mode != "Firmware"))
                    throw new InvalidOperationException("This migration requires valid imported firmware-mode rules");
                engine.Open();
                engine.Read(); Thread.Sleep(1500);
                var before = engine.Read();
                double Value(IReadOnlyDictionary<string, SensorReading> readings, string id, string type)
                {
                    if (!readings.TryGetValue(id, out var s) || s.Type != type || s.Value is not double value || !double.IsFinite(value))
                        throw new InvalidOperationException("Missing feedback: " + id);
                    return value;
                }
                var testRules = saved.Fans.Select(rule =>
                {
                    if (!before.TryGetValue(rule.ControlId, out var output) || !output.CanControl || rule.RpmSensorId is null)
                        throw new InvalidOperationException("Unmapped imported output: " + rule.Name);
                    _ = Value(before, rule.RpmSensorId, "Fan");
                    string source;
                    if (rule.ControlId.StartsWith("/gpu-nvidia/", StringComparison.Ordinal))
                        source = before.Values.Single(s => s.Hardware == output.Hardware && s.Type == "Temperature" && s.Name == "GPU Core").Id;
                    else if (rule.ControlId.StartsWith("/lpc/", StringComparison.Ordinal))
                        source = before.Values.Single(s => s.Id.StartsWith("/amdcpu/", StringComparison.Ordinal) && s.Name == "Core (Tctl/Tdie)").Id;
                    else throw new InvalidOperationException("Unsupported migration channel: " + rule.ControlId);
                    var temperature = Value(before, source, "Temperature");
                    if (temperature <= 0 || temperature >= 85) throw new InvalidOperationException("Wait for a cool idle system before testing");
                    return rule with {Mode = "Manual", ManualPercent = 100, TemperatureSensorIds = [source]};
                }).ToArray();
                samples.Add(new {Phase = "Before", Sensors = before.Values});
                // Increasing to full speed never requests a low duty cycle on an unknown pump/fan.
                IReadOnlyDictionary<string, SensorReading> atFull = before;
                for (var i = 0; i < 15; i++)
                {
                    atFull = engine.Read();
                    foreach (var rule in testRules)
                        if (Value(atFull, rule.TemperatureSensorIds[0], "Temperature") >= 90)
                            throw new InvalidOperationException("Temperature rose too high during test");
                    engine.Apply(testRules, atFull);
                    Thread.Sleep(1000);
                }
                atFull = engine.Read();
                foreach (var rule in testRules)
                {
                    var rpm = Value(atFull, rule.RpmSensorId!, "Fan");
                    var expected = rule.Calibration.LastOrDefault(p => p.Percent == 100)?.Rpm ?? 0;
                    if (rpm < 200 || (expected > 0 && rpm < expected * 0.8))
                        throw new InvalidOperationException("Full-speed RPM check failed: " + rule.Name);
                }
                samples.Add(new {Phase = "FullSpeed", Sensors = atFull.Values});
                engine.Suspend(true);
                if (engine.Errors.Count != 0) throw new InvalidOperationException(string.Join("; ", engine.Errors));
                IReadOnlyDictionary<string, SensorReading> released = atFull;
                bool returned = false;
                for (var i = 0; i < 45; i++)
                {
                    Thread.Sleep(1000);
                    released = engine.Read();
                    returned = testRules.All(r => Value(released, r.ControlId, "Control") <= Value(before, r.ControlId, "Control") + 15);
                    if (returned && i >= 10) break;
                }
                if (!returned) throw new InvalidOperationException("Cannot verify return to firmware duty cycles");
                foreach (var rule in testRules.Where(r => r.ControlId.StartsWith("/lpc/", StringComparison.Ordinal)))
                    if (Value(released, rule.RpmSensorId!, "Fan") < 200) throw new InvalidOperationException("Motherboard fan stopped after release");
                samples.Add(new {Phase = "FirmwareReleased", Sensors = released.Values});
                // The file lock also proves the original RGB writer has exited.
                using var lights = new LightingEngine();
                lights.Apply(saved);
                samples.Add(new {Phase = "LightingRestored", Devices = lights.ReadDevices()});
                passed = true;
            }
            catch (Exception exception) { error = exception.ToString(); }
        }
        finally
        {
            mutex.ReleaseMutex();
            SettingsStore.AtomicWrite(reportPath, JsonSerializer.SerializeToUtf8Bytes(new
            {Timestamp = DateTimeOffset.UtcNow, Passed = passed, Error = error, Samples = samples}, new JsonSerializerOptions {WriteIndented = true}));
        }
        if (!passed) throw new InvalidOperationException(error ?? "Handover test failed");
    }
}
