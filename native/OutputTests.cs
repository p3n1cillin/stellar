namespace CoolingLighting;

public static class OutputTests
{
    private sealed class FakeOutput : IFanOutput
    {
        public float Minimum { get; init; }
        public float Maximum { get; init; } = 100;
        public List<float> Writes { get; } = [];
        public int Releases { get; private set; }
        public int WriteFailures { get; set; }
        public bool ReleaseFailure { get; set; }
        public void SetSpeed(float value)
        {
            Writes.Add(value);
            if (WriteFailures-- > 0) throw new InvalidOperationException("Injected write failure");
        }
        public void RestoreFirmware()
        {
            Releases++;
            if (ReleaseFailure) throw new InvalidOperationException("Injected release failure");
        }
    }

    public static void Run()
    {
        var count = 0;
        void Check(bool condition, string name) { if (!condition) throw new Exception("FAIL: " + name); count++; }
        void ExpectFailure(Action action)
        {
            try { action(); } catch (Exception exception) when (exception is InvalidOperationException or ArgumentException) { return; }
            throw new Exception("Expected output failure");
        }
        var errors = new List<string>();
        var now = DateTimeOffset.UtcNow;
        var readings = new Dictionary<string, SensorReading> { ["cpu"] = new("cpu", "CPU", "Package", "Temperature", 40, now, false) };
        var rule = new FanRule { ControlId = "a", Mode = "Manual", ManualPercent = 50, TemperatureSensorIds = ["cpu"] };
        var a = new FakeOutput(); var b = new FakeOutput();
        var outputs = new Dictionary<string, IFanOutput> { ["a"] = a, ["b"] = b };
        var engine = new FanOutputs(errors.Add);
        ExpectFailure(() => engine.Apply([rule], readings, outputs, false, false, now));
        Check(a.Writes.Count == 0, "non-admin cannot write output");
        ExpectFailure(() => engine.Apply([rule], readings, outputs, true, true, now));
        Check(a.Writes.Count == 0 && a.Releases == 0, "competing writer blocks new ownership");
        engine.Apply([rule with {Mode = "Firmware"}], readings, outputs, false, true, now);
        Check(a.Writes.Count == 0 && a.Releases == 0, "firmware preview never writes unowned fans");
        engine.Apply([rule], readings, outputs, true, false, now);
        Check(a.Writes.SequenceEqual(new float[] {50}), "valid request writes speed");
        ExpectFailure(() => engine.Apply([rule], readings, outputs, true, true, now));
        Check(a.Releases == 1 && a.Writes.Count == 1, "new competing writer releases existing ownership");
        engine.Apply([rule], readings, outputs, true, false, now);
        engine.Suspend(true);
        Check(a.Releases == 2, "suspend releases owned fan");
        engine.Apply([rule], readings, outputs, true, false, now);
        Check(a.Writes.Count == 2, "suspended engine blocks writes");
        engine.Suspend(false);
        engine.Apply([rule], readings, outputs, true, false, now);
        Check(a.Writes.Count == 3, "resume restores independent control");
        engine.Apply([], readings, outputs, true, false, now);
        Check(a.Releases == 3, "removed rule returns output to firmware");
        engine.Apply([rule], readings, outputs, true, false, now);
        engine.Apply([rule with {Mode = "Firmware"}], readings, outputs, true, false, now);
        Check(a.Releases == 4, "firmware rule releases output");
        var limited = new FakeOutput {Minimum = 60, Maximum = 80};
        engine.Apply([rule], readings, new Dictionary<string, IFanOutput> { ["a"] = limited }, true, false, now);
        Check(limited.Writes.Last() == 60, "hardware lower bound clamps command");
        engine.Apply([rule with {ManualPercent = 100}], readings, new Dictionary<string, IFanOutput> { ["a"] = limited }, true, false, now);
        Check(limited.Writes.Last() == 80, "hardware upper bound clamps command");
        engine.ReleaseAll(); engine.ReleaseAll();
        Check(limited.Releases == 1, "release is idempotent after success");
        var faulty = new FakeOutput {WriteFailures = 1};
        var partial = new FanOutputs(errors.Add);
        var fresh = new FakeOutput();
        var pair = new Dictionary<string, IFanOutput> { ["a"] = fresh, ["b"] = faulty };
        ExpectFailure(() => partial.Apply([rule, rule with {ControlId = "b"}], readings, pair, true, false, now));
        Check(fresh.Writes.Last() == 100 && faulty.Writes.Last() == 100, "partial update commands full cooling on all owned fans");
        var newer = readings.ToDictionary(k => k.Key, k => k.Value with {UpdatedAt = now.AddSeconds(1)});
        partial.Apply([rule, rule with {ControlId = "b"}], newer, pair, true, false, now.AddSeconds(1));
        Check(fresh.Writes.Last() == 95, "recovery ramps down from emergency output");
        partial.ReleaseAll();
        Check(faulty.Releases == 1, "failed first write still claims for release");
        var lost = new FanOutputs(errors.Add); var old = new FakeOutput();
        lost.Apply([rule], readings, new Dictionary<string, IFanOutput> {["a"] = old}, true, false, now);
        ExpectFailure(() => lost.Apply([rule], readings, new Dictionary<string, IFanOutput>(), true, false, now));
        Check(old.Writes.Last() == 100, "disappeared output retains emergency release reference");
        old.ReleaseFailure = true; lost.ReleaseAll();
        Check(errors.Any(e => e.Contains("Release a")), "release errors are reported");
        old.ReleaseFailure = false; lost.ReleaseAll();
        Check(old.Releases == 2, "failed release retries retained output");
        var invalid = new FakeOutput {Minimum = float.NaN};
        ExpectFailure(() => new FanOutputs(errors.Add).Apply([rule], readings,
            new Dictionary<string, IFanOutput> {["a"] = invalid}, true, false, now));
        Check(invalid.Writes.Count == 0, "invalid hardware range never writes NaN");
        var validate = new FakeOutput();
        ExpectFailure(() => new FanOutputs(errors.Add).Apply([rule, rule with {ControlId = "b", Points = []}], readings,
            new Dictionary<string, IFanOutput> {["a"] = validate}, true, false, now));
        Check(validate.Writes.Count == 0, "entire ruleset validates before writes");
        var startup = new FanOutputs(errors.Add); var restarting = new FakeOutput();
        var low = rule with {ManualPercent = 30, MinimumPercent = 30};
        var startOutputs = new Dictionary<string, IFanOutput> {["a"] = restarting};
        startup.Apply([low], readings, startOutputs, true, false, now);
        startup.Apply([], readings, startOutputs, true, false, now);
        startup.Apply([low], readings, startOutputs, true, false, now);
        Check(restarting.Writes.Last() == 40, "reacquired fan gets startup floor again");
        startup.ReleaseAll();
        Console.WriteLine($"PASS: {count} fan ownership and failure-path checks (fake hardware only)");
    }
}
