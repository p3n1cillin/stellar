using System.Diagnostics;
using System.Security.Principal;
using LibreHardwareMonitor.Hardware;

namespace CoolingLighting;

public sealed class HardwareEngine : IDisposable
{
    private readonly Computer _computer = new()
    {
        IsCpuEnabled = true, IsGpuEnabled = true, IsMotherboardEnabled = true,
        IsControllerEnabled = false, IsMemoryEnabled = false, IsStorageEnabled = false
    };
    private readonly Dictionary<string, IFanOutput> _controls = [];
    private readonly FanOutputs _outputs;
    private readonly object _gate = new();
    private bool _disposed;
    public bool Administrator => new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
    public List<string> Errors { get; } = [];

    public HardwareEngine() => _outputs = new FanOutputs(message => { Errors.Add(message); Diagnostics.Log(message); });
    public string[] ErrorSnapshot() { lock (_gate) return Errors.ToArray(); }

    private sealed class HardwareOutput(IControl control) : IFanOutput
    {
        public float Minimum => control.MinSoftwareValue;
        public float Maximum => control.MaxSoftwareValue;
        public void SetSpeed(float percent) => control.SetSoftware(percent);
        public void RestoreFirmware() => control.SetDefault();
    }

    public static bool FanControlRunning()
    {
        var processes = Process.GetProcessesByName("FanControl");
        try { return processes.Length > 0; }
        finally { foreach (var process in processes) process.Dispose(); }
    }

    public void Open() { lock (_gate) { ObjectDisposedException.ThrowIf(_disposed, this); _computer.Open(); } }

    public IReadOnlyDictionary<string, SensorReading> Read()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            Errors.Clear();
            var readings = new Dictionary<string, SensorReading>();
            _controls.Clear();
            foreach (var hardware in _computer.Hardware) ReadHardware(hardware, readings);
            return readings;
        }
    }

    private void ReadHardware(IHardware hardware, Dictionary<string, SensorReading> readings)
    {
        try
        {
            hardware.Update();
            var now = DateTimeOffset.UtcNow;
            foreach (var sensor in hardware.Sensors)
            {
                if (sensor.SensorType is not (SensorType.Temperature or SensorType.Fan or SensorType.Control or SensorType.Load)) continue;
                var id = sensor.Identifier.ToString();
                double? value = sensor.Value;
                if (sensor.SensorType == SensorType.Temperature && value <= 0) value = null;
                readings[id] = new(id, hardware.Name, sensor.Name, sensor.SensorType.ToString(), value, now, sensor.Control is not null);
                if (sensor.Control is not null) _controls[id] = new HardwareOutput(sensor.Control);
            }
        }
        catch (Exception exception) { Errors.Add($"{hardware.Name}: {exception.Message}"); }
        foreach (var child in hardware.SubHardware) ReadHardware(child, readings);
    }

    public Dictionary<string, FanDecision> Apply(IReadOnlyList<FanRule> rules, IReadOnlyDictionary<string, SensorReading> readings)
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            return _outputs.Apply(rules, readings, _controls, Administrator, FanControlRunning(), DateTimeOffset.UtcNow);
        }
    }

    public void Suspend(bool suspended)
    {
        lock (_gate)
        {
            if (!_disposed) _outputs.Suspend(suspended);
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed) return;
            _disposed = true;
            _outputs.ReleaseAll();
            _computer.Close();
        }
    }
}
