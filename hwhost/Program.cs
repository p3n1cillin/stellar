// stellar-hw: line-delimited JSON hardware host for Stellar.
//
// Requests on stdin, one JSON object per line:
//   {"cmd":"read"}                         -> sensors, controls, admin flag, errors
//   {"cmd":"set","id":"...","percent":55}  -> software duty cycle
//   {"cmd":"release","id":"..."}           -> return one output to firmware
//   {"cmd":"release_all"} / {"cmd":"quit"}
//
// Safety: every output this process has touched is returned to firmware
// control when stdin closes (Stellar exited or crashed), on "quit", on process
// exit, and when no request arrives for WatchdogSeconds (Stellar hung).
using System.Security.Principal;
using System.Text.Json;
using System.Text.Json.Nodes;
using LibreHardwareMonitor.Hardware;

const int WatchdogSeconds = 10;
var computer = new Computer
{
    IsCpuEnabled = true, IsGpuEnabled = true, IsMotherboardEnabled = true,
    IsControllerEnabled = true, IsMemoryEnabled = false, IsStorageEnabled = true,
};
var gate = new object();
var controls = new Dictionary<string, IControl>();
var touched = new HashSet<string>();
var lastRequest = DateTime.UtcNow;
var admin = new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
var stdout = Console.Out;

void ReleaseAll()
{
    lock (gate)
    {
        foreach (var id in touched.ToArray())
        {
            try { if (controls.TryGetValue(id, out var c)) c.SetDefault(); }
            catch (Exception e) { Console.Error.WriteLine($"release {id}: {e.Message}"); }
        }
        touched.Clear();
    }
}

AppDomain.CurrentDomain.ProcessExit += (_, _) => { ReleaseAll(); try { computer.Close(); } catch { } };
computer.Open();

var watchdog = new Thread(() =>
{
    while (true)
    {
        Thread.Sleep(1000);
        bool stale;
        lock (gate) stale = touched.Count > 0 && DateTime.UtcNow - lastRequest > TimeSpan.FromSeconds(WatchdogSeconds);
        if (stale) { Console.Error.WriteLine("watchdog: controller silent, returning outputs to firmware"); ReleaseAll(); }
    }
}) { IsBackground = true };
watchdog.Start();

JsonObject Read()
{
    var sensors = new JsonArray();
    var outs = new JsonObject();
    var errors = new JsonArray();
    var now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
    controls.Clear();
    void Visit(IHardware hw)
    {
        try
        {
            hw.Update();
            foreach (var s in hw.Sensors)
            {
                if (s.SensorType is not (SensorType.Temperature or SensorType.Fan or SensorType.Control
                    or SensorType.Load or SensorType.Power or SensorType.Clock)) continue;
                var id = s.Identifier.ToString();
                double? value = s.Value;
                if (value is { } v && !double.IsFinite(v)) value = null;
                if (s.SensorType == SensorType.Temperature && value <= 0) value = null;
                sensors.Add(new JsonObject
                {
                    ["id"] = id, ["hardware"] = hw.Name, ["hardware_type"] = hw.HardwareType.ToString(),
                    ["name"] = s.Name, ["kind"] = s.SensorType.ToString(), ["value"] = value,
                    ["updated_ms"] = now, ["can_control"] = s.Control is not null,
                });
                if (s.Control is { } c)
                {
                    controls[id] = c;
                    outs[id] = new JsonObject
                    {
                        ["min"] = c.MinSoftwareValue, ["max"] = c.MaxSoftwareValue,
                        ["software"] = c.ControlMode == ControlMode.Software,
                    };
                }
            }
        }
        catch (Exception e) { errors.Add($"{hw.Name}: {e.Message}"); }
        foreach (var sub in hw.SubHardware) Visit(sub);
    }
    foreach (var hw in computer.Hardware) Visit(hw);
    return new JsonObject { ["ok"] = true, ["admin"] = admin, ["sensors"] = sensors, ["controls"] = outs, ["errors"] = errors };
}

void Reply(JsonObject o) { lock (stdout) { stdout.WriteLine(o.ToJsonString()); stdout.Flush(); } }

string? line;
while ((line = Console.In.ReadLine()) is not null)
{
    if (string.IsNullOrWhiteSpace(line)) continue;
    try
    {
        var req = JsonNode.Parse(line)!.AsObject();
        var cmd = (string?)req["cmd"];
        lock (gate)
        {
            lastRequest = DateTime.UtcNow;
            switch (cmd)
            {
                case "read": Reply(Read()); break;
                case "ping": Reply(new JsonObject { ["ok"] = true }); break;
                case "set":
                {
                    var id = (string)req["id"]!;
                    var pct = (double)req["percent"]!;
                    if (!controls.TryGetValue(id, out var c)) throw new InvalidOperationException($"Unknown output {id}");
                    if (!double.IsFinite(pct)) throw new ArgumentException("percent must be finite");
                    pct = Math.Clamp(pct, c.MinSoftwareValue, c.MaxSoftwareValue);
                    touched.Add(id); // claim before writing so a failed write still gets released
                    c.SetSoftware((float)pct);
                    Reply(new JsonObject { ["ok"] = true });
                    break;
                }
                case "release":
                {
                    var id = (string)req["id"]!;
                    if (controls.TryGetValue(id, out var c)) c.SetDefault();
                    touched.Remove(id);
                    Reply(new JsonObject { ["ok"] = true });
                    break;
                }
                case "release_all": ReleaseAll(); Reply(new JsonObject { ["ok"] = true }); break;
                case "quit": ReleaseAll(); Reply(new JsonObject { ["ok"] = true }); goto done;
                default: throw new ArgumentException($"Unknown command {cmd}");
            }
        }
    }
    catch (Exception e) { Reply(new JsonObject { ["ok"] = false, ["error"] = e.Message }); }
}
done:
ReleaseAll();
computer.Close();
