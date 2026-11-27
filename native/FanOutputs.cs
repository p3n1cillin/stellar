namespace CoolingLighting;

public interface IFanOutput
{
    float Minimum { get; }
    float Maximum { get; }
    void SetSpeed(float percent);
    void RestoreFirmware();
}

// Hardware-independent ownership boundary. Tests use fake outputs, never real fans.
public sealed class FanOutputs
{
    private readonly Dictionary<string, IFanOutput> _owned = [];
    private readonly CoolingPolicy _policy = new();
    private readonly Action<string> _error;
    private bool _suspended;

    public FanOutputs(Action<string> error) => _error = error;

    public Dictionary<string, FanDecision> Apply(IReadOnlyList<FanRule> rules,
        IReadOnlyDictionary<string, SensorReading> readings, IReadOnlyDictionary<string, IFanOutput> outputs,
        bool administrator, bool competingWriter, DateTimeOffset now)
    {
        if (_suspended) return new();
        if (competingWriter && _owned.Count > 0) ReleaseAll();
        if (rules.Any(r => r.Mode != "Firmware"))
        {
            if (competingWriter) throw new InvalidOperationException("Fan Control is still running. Replacement outputs remain disabled to prevent competing fan writers.");
            if (!administrator) throw new InvalidOperationException("Administrator access is required for fan outputs");
        }
        var decisions = new Dictionary<string, FanDecision>();
        try
        {
            // Validate the whole update before the first write.
            foreach (var rule in rules) rule.Validate();
            if (rules.Select(r => r.ControlId).Distinct().Count() != rules.Count)
                throw new ArgumentException("Duplicate fan outputs");
            var ids = rules.Select(r => r.ControlId).ToHashSet();
            foreach (var old in _owned.Keys.Where(id => !ids.Contains(id)).ToArray()) Release(old);
            foreach (var rule in rules)
            {
                var decision = _policy.Decide(rule, readings, now);
                decisions[rule.ControlId] = decision;
                if (decision.Percent is null) { Release(rule.ControlId); continue; }
                if (!outputs.TryGetValue(rule.ControlId, out var output))
                    throw new InvalidOperationException($"Fan output disappeared: {rule.Name}");
                if (!float.IsFinite(output.Minimum) || !float.IsFinite(output.Maximum)
                    || output.Minimum < 0 || output.Maximum > 100 || output.Minimum > output.Maximum)
                    throw new InvalidOperationException($"Invalid speed range: {rule.Name}");
                // Claim before writing: even a failed write must get a release attempt.
                _owned[rule.ControlId] = output;
                output.SetSpeed((float)Math.Clamp(decision.Percent.Value, output.Minimum, output.Maximum));
            }
            return decisions;
        }
        catch
        {
            foreach (var pair in _owned)
                try
                {
                    pair.Value.SetSpeed(pair.Value.Maximum);
                    _policy.RecordEmergencyOutput(pair.Key, pair.Value.Maximum, now);
                }
                catch (Exception exception) { _error($"Emergency cooling {pair.Key}: {exception.Message}"); }
            throw;
        }
    }

    private void Release(string id)
    {
        if (!_owned.TryGetValue(id, out var output)) return;
        output.RestoreFirmware();
        _owned.Remove(id);
        _policy.Forget(id);
    }

    public void ReleaseAll()
    {
        foreach (var id in _owned.Keys.ToArray())
            try { Release(id); }
            catch (Exception exception) { _error($"Release {id}: {exception.Message}"); }
    }

    public void Suspend(bool suspended)
    {
        _suspended = suspended;
        ReleaseAll();
        _policy.Reset();
    }
}
