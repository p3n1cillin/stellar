namespace CoolingLighting;

public record CurvePoint(double Temperature, double Percent);
public record CalibrationPoint(double Percent, double Rpm);

public sealed record FanRule
{
    public string ControlId { get; init; } = "";
    public string Name { get; init; } = "";
    public string? RpmSensorId { get; init; }
    public CalibrationPoint[] Calibration { get; init; } = [];
    public string[] TemperatureSensorIds { get; init; } = [];
    public string Mode { get; init; } = "Firmware";
    public double ManualPercent { get; init; } = 70;
    public double MinimumPercent { get; init; } = 30;
    public double StartPercent { get; init; } = 40;
    public double CriticalTemperature { get; init; } = 85;
    public double Hysteresis { get; init; } = 2;
    public double RampDownPerSecond { get; init; } = 5;
    public CurvePoint[] Points { get; init; } = [new(35, 40), new(55, 55), new(70, 80), new(85, 100)];

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(ControlId)) throw new ArgumentException("Select a fan output");
        if (TemperatureSensorIds is null || TemperatureSensorIds.Any(string.IsNullOrWhiteSpace)
            || Points is null || Points.Any(p => p is null) || Calibration is null
            || Calibration.Any(p => p is null || !double.IsFinite(p.Percent) || p.Percent < 0 || p.Percent > 100
                || !double.IsFinite(p.Rpm) || p.Rpm < 0 || p.Rpm > 30000))
            throw new ArgumentException("Invalid sensor or curve data");
        if (Mode is not ("Firmware" or "Curve" or "Manual")) throw new ArgumentException("Unknown fan mode");
        foreach (var value in new[] {ManualPercent, MinimumPercent, StartPercent})
            if (!double.IsFinite(value) || value < 0 || value > 100) throw new ArgumentException("Fan speeds must be 0–100%");
        if (!double.IsFinite(CriticalTemperature) || CriticalTemperature < 50 || CriticalTemperature > 100)
            throw new ArgumentException("Critical temperature must be 50–100°C");
        if (!double.IsFinite(Hysteresis) || Hysteresis < 0 || Hysteresis > 10
            || !double.IsFinite(RampDownPerSecond) || RampDownPerSecond <= 0 || RampDownPerSecond > 100)
            throw new ArgumentException("Invalid smoothing settings");
        if (Mode is "Curve" or "Manual" && TemperatureSensorIds.Length == 0)
            throw new ArgumentException("A temperature sensor is required for the thermal failsafe");
        if (Points.Length < 2 || Points.Any(p => !double.IsFinite(p.Temperature) || !double.IsFinite(p.Percent)
               || p.Percent < 0 || p.Percent > 100 || p.Temperature < 0 || p.Temperature > 110))
            throw new ArgumentException("Invalid curve points");
        for (var i = 1; i < Points.Length; i++)
            if (Points[i].Temperature <= Points[i-1].Temperature || Points[i].Percent < Points[i-1].Percent)
                throw new ArgumentException("Curve temperatures must increase and fan speeds cannot decrease");
    }
}

public record SensorReading(string Id, string Hardware, string Name, string Type, double? Value,
                            DateTimeOffset UpdatedAt, bool CanControl);
public record FanDecision(double? Percent, string Reason, bool Emergency);

public sealed class CoolingPolicy
{
    private readonly Dictionary<string, (double Percent, double Temperature, DateTimeOffset Time)> _last = [];
    private readonly Dictionary<string, DateTimeOffset> _lowRpmSince = [];

    public static double Interpolate(CurvePoint[] points, double temperature)
    {
        if (temperature <= points[0].Temperature) return points[0].Percent;
        for (var i = 1; i < points.Length; i++)
        {
            var lo = points[i-1];
            var hi = points[i];
            if (temperature <= hi.Temperature)
                return lo.Percent + (hi.Percent - lo.Percent) * (temperature - lo.Temperature) / (hi.Temperature - lo.Temperature);
        }
        return points[^1].Percent;
    }

    public FanDecision Decide(FanRule rule, IReadOnlyDictionary<string, SensorReading> sensors, DateTimeOffset now)
    {
        rule.Validate();
        if (rule.Mode == "Firmware")
        {
            _last.Remove(rule.ControlId);
            _lowRpmSince.Remove(rule.ControlId);
            return new(null, "Firmware control", false);
        }
        var inputs = rule.TemperatureSensorIds.Select(id => sensors.GetValueOrDefault(id)).ToArray();
        if (inputs.Any(s => s is null || s.Type != "Temperature" || s.Value is null
              || !double.IsFinite(s.Value.Value) || s.Value < -10 || s.Value > 130
              || now - s.UpdatedAt > TimeSpan.FromSeconds(5) || s.UpdatedAt > now + TimeSpan.FromSeconds(1)))
            return Remember(rule, 100, double.NaN, now, "Temperature unavailable — full cooling", true);
        var temperature = inputs.Max(s => s!.Value!.Value);
        if (temperature >= rule.CriticalTemperature)
            return Remember(rule, 100, temperature, now, "Temperature limit — full cooling", true);
        var target = Math.Max(rule.MinimumPercent,
            rule.Mode == "Manual" ? rule.ManualPercent : Interpolate(rule.Points, temperature));
        if (!string.IsNullOrWhiteSpace(rule.RpmSensorId) && target > 0)
        {
            var rpm = sensors.GetValueOrDefault(rule.RpmSensorId);
            if (rpm is null || rpm.Type != "Fan" || rpm.Value is null || !double.IsFinite(rpm.Value.Value)
                || rpm.Value < 0 || rpm.Value > 30000 || now - rpm.UpdatedAt > TimeSpan.FromSeconds(5)
                || rpm.UpdatedAt > now + TimeSpan.FromSeconds(1))
                return Remember(rule, 100, double.NaN, now, "RPM feedback unavailable — full cooling", true);
            if (rpm.Value < 100)
            {
                if (!_lowRpmSince.TryGetValue(rule.ControlId, out var since)) _lowRpmSince[rule.ControlId] = now;
                else if (now - since >= TimeSpan.FromSeconds(15))
                    return Remember(rule, 100, double.NaN, now, "Fan may be stalled — full cooling", true);
            }
            else _lowRpmSince.Remove(rule.ControlId);
        }
        else _lowRpmSince.Remove(rule.ControlId);
        if (_last.TryGetValue(rule.ControlId, out var previous))
        {
            // Increases are immediate. Only decreases use hysteresis and slew limiting.
            if (target < previous.Percent)
            {
                if (double.IsFinite(previous.Temperature) && temperature > previous.Temperature - rule.Hysteresis)
                    target = previous.Percent;
                else
                    target = Math.Max(target, previous.Percent - rule.RampDownPerSecond * Math.Clamp((now - previous.Time).TotalSeconds, 0, 5));
            }
        }
        else
            target = Math.Max(target, rule.StartPercent);
        // Keep a stable temperature anchor while holding for hysteresis.
        var anchor = _last.TryGetValue(rule.ControlId, out var last) && target == last.Percent
                     && temperature < last.Temperature ? last.Temperature : temperature;
        return Remember(rule, Math.Clamp(target, 0, 100), anchor, now,
                        rule.Mode == "Manual" ? "Manual with thermal protection" : "Temperature curve", false);
    }

    private FanDecision Remember(FanRule rule, double percent, double temperature, DateTimeOffset time, string reason, bool emergency)
    {
        _last[rule.ControlId] = (percent, temperature, time);
        return new(percent, reason, emergency);
    }

    public void Reset() { _last.Clear(); _lowRpmSince.Clear(); }
    public void Forget(string controlId) { _last.Remove(controlId); _lowRpmSince.Remove(controlId); }
    public void RecordEmergencyOutput(string controlId, double percent, DateTimeOffset now)
        => _last[controlId] = (percent, double.NaN, now);
}
