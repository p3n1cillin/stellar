namespace CoolingLighting;

public static class PolicyTests
{
    public static void Run()
    {
        var now = DateTimeOffset.UtcNow;
        var rule = new FanRule { ControlId = "fan", Mode = "Curve", TemperatureSensorIds = ["cpu"],
                                Points = [new(30, 30), new(80, 100)], Hysteresis = 2, RampDownPerSecond = 5 };
        Dictionary<string, SensorReading> Input(double? value, DateTimeOffset? time = null) =>
            new() { ["cpu"] = new("cpu", "CPU", "Package", "Temperature", value, time ?? now, false) };
        var count = 0;
        void Check(bool condition, string name) { if (!condition) throw new Exception("FAIL: " + name); count++; }
        Check(CoolingPolicy.Interpolate(rule.Points, 55) == 65, "linear interpolation");
        Check(CoolingPolicy.Interpolate(rule.Points, 10) == 30, "lower bound");
        Check(CoolingPolicy.Interpolate(rule.Points, 100) == 100, "upper bound");
        Check(new CoolingPolicy().Decide(rule, Input(null), now).Emergency, "missing sensor failsafe");
        Check(new CoolingPolicy().Decide(rule, new Dictionary<string, SensorReading>(), now).Percent == 100, "absent sensor full speed");
        Check(new CoolingPolicy().Decide(rule, Input(40, now.AddSeconds(-6)), now).Percent == 100, "stale sensor full speed");
        Check(new CoolingPolicy().Decide(rule, Input(double.NaN), now).Percent == 100, "nonfinite sensor full speed");
        Check(new CoolingPolicy().Decide(rule, Input(90), now).Emergency, "critical temperature full speed");
        Check(new CoolingPolicy().Decide(rule with { Mode = "Manual", ManualPercent = 35 }, Input(90), now).Percent == 100, "manual mode thermal override");
        Check(new CoolingPolicy().Decide(rule with { Mode = "Firmware" }, Input(null), now).Percent is null, "firmware release");
        Check(new CoolingPolicy().Decide(rule, Input(20), now).Percent == 40, "startup floor");
        var policy = new CoolingPolicy();
        policy.Decide(rule, Input(70), now);
        Check(policy.Decide(rule, Input(50, now.AddSeconds(1)), now.AddSeconds(1)).Percent == 81, "ramp-down rate");
        Check(policy.Decide(rule, Input(90, now.AddSeconds(2)), now.AddSeconds(2)).Percent == 100, "emergency bypasses smoothing");
        var hysteresis = new CoolingPolicy();
        var initial = hysteresis.Decide(rule, Input(60), now);
        Check(hysteresis.Decide(rule, Input(59, now.AddSeconds(1)), now.AddSeconds(1)).Percent == initial.Percent, "hysteresis holds");
        Check(hysteresis.Decide(rule, Input(57, now.AddSeconds(2)), now.AddSeconds(2)).Percent < initial.Percent, "hysteresis eventually releases");
        try { (rule with { Points = [new(80, 30), new(30, 100)] }).Validate(); throw new Exception("Invalid curve accepted"); }
        catch (ArgumentException) { count++; }
        try { (rule with { TemperatureSensorIds = [] }).Validate(); throw new Exception("Unguarded manual accepted"); }
        catch (ArgumentException) { count++; }
        var paired = rule with { RpmSensorId = "rpm" };
        Dictionary<string, SensorReading> PairedInput(double speed, int elapsed)
        {
            var time = now.AddSeconds(elapsed);
            var data = Input(40, time);
            data["rpm"] = new("rpm", "CPU", "Fan", "Fan", speed, time, false);
            return data;
        }
        Check(new CoolingPolicy().Decide(paired, Input(40), now).Emergency, "missing RPM feedback full speed");
        var stalled = new CoolingPolicy();
        Check(!stalled.Decide(paired, PairedInput(0, 0), now).Emergency, "RPM spin-up grace period");
        Check(!stalled.Decide(paired, PairedInput(0, 14), now.AddSeconds(14)).Emergency, "RPM grace lasts fifteen seconds");
        Check(stalled.Decide(paired, PairedInput(0, 15), now.AddSeconds(15)).Percent == 100, "stalled fan full speed");
        Check(!stalled.Decide(paired, PairedInput(800, 16), now.AddSeconds(16)).Emergency, "recovered RPM clears stall");
        Check(!new CoolingPolicy().Decide(paired with {Mode = "Manual", ManualPercent = 0, MinimumPercent = 0, StartPercent = 0}, PairedInput(0, 0), now).Emergency, "intentional zero speed not a stall");
        Check(new CoolingPolicy().Decide(rule with {TemperatureSensorIds = ["cpu", "missing"]}, Input(40), now).Emergency, "all selected temperatures required");
        var mixed = Input(40);
        mixed["gpu"] = new("gpu", "GPU", "Core", "Temperature", 70, now, false);
        Check(new CoolingPolicy().Decide(rule with {TemperatureSensorIds = ["cpu", "gpu"]}, mixed, now).Percent == 86, "highest selected temperature drives mixed curve");
        try { (rule with { Points = null! }).Validate(); throw new Exception("Null curve accepted"); }
        catch (ArgumentException) { count++; }
        Console.WriteLine($"PASS: {count} cooling policy checks");
    }
}
