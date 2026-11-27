namespace CoolingLighting;

// Defaults must never silently choose an unlabelled motherboard probe.
public static class TemperatureSelection
{
    public static SensorReading? Cpu(IEnumerable<SensorReading> readings) => readings
        .Where(s => s.Type == "Temperature" && (s.Id.StartsWith("/amdcpu/", StringComparison.OrdinalIgnoreCase)
            || s.Id.StartsWith("/intelcpu/", StringComparison.OrdinalIgnoreCase)))
        .OrderBy(s => s.Name.Equals("CPU Package", StringComparison.OrdinalIgnoreCase) ? 0
            : s.Name.Contains("Tctl/Tdie", StringComparison.OrdinalIgnoreCase) ? 1 : 2)
        .FirstOrDefault(s => s.Name.Contains("Package", StringComparison.OrdinalIgnoreCase)
            || s.Name.Contains("Tctl", StringComparison.OrdinalIgnoreCase));

    public static SensorReading? Gpu(IEnumerable<SensorReading> readings, string? hardware = null) => readings
        .FirstOrDefault(s => s.Type == "Temperature" && s.Id.StartsWith("/gpu-", StringComparison.OrdinalIgnoreCase)
            && (hardware is null || s.Hardware == hardware) && s.Name.Equals("GPU Core", StringComparison.OrdinalIgnoreCase));

    public static SensorReading? ForOutput(IEnumerable<SensorReading> readings, SensorReading? output)
    {
        var sensors = readings.ToArray();
        return output?.Id.StartsWith("/gpu-", StringComparison.OrdinalIgnoreCase) == true
            ? Gpu(sensors, output.Hardware) : Cpu(sensors);
    }

    public static void Test()
    {
        SensorReading S(string id, string name, string hardware = "CPU", string type = "Temperature") =>
            new(id, hardware, name, type, 50, DateTimeOffset.UtcNow, type == "Control");
        var probe = S("/lpc/chip/temperature/0", "Temperature #1", "Motherboard");
        var amd = S("/amdcpu/0/temperature/0", "Core (Tctl/Tdie)");
        var intel = S("/intelcpu/0/temperature/0", "CPU Package");
        var gpu = S("/gpu-nvidia/0/temperature/0", "GPU Core", "GPU");
        var board = S("/lpc/chip/control/0", "Fan #1", "Motherboard", "Control");
        var gpuOutput = S("/gpu-nvidia/0/control/0", "Fan", "GPU", "Control");
        var count = 0;
        void Check(bool ok, string name) { if (!ok) throw new Exception("FAIL: " + name); count++; }
        Check(Cpu([probe, amd]) == amd, "AMD primary temperature");
        Check(Cpu([probe, intel]) == intel, "Intel primary temperature");
        Check(ForOutput([probe, amd, gpu], board) == amd, "motherboard defaults to CPU not generic probe");
        Check(ForOutput([probe, amd, gpu], gpuOutput) == gpu, "GPU defaults to its own core");
        Check(ForOutput([probe, gpu], board) is null, "no unrelated temperature fallback");
        Check(ForOutput([amd, gpu with {Hardware = "Other GPU"}], gpuOutput) is null, "no cross-GPU fallback");
        Check(Cpu([amd with {Value = null}])?.Value is null, "unavailable remains unavailable");
        Check(Gpu([gpu with {Name = "GPU Hot Spot"}]) is null, "hotspot is not silently substituted");
        Console.WriteLine($"PASS: {count} temperature selection checks");
    }
}
