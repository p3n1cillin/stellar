using OpenRGB.NET;
using Color = OpenRGB.NET.Color;
using System.IO;
using System.Diagnostics;

namespace CoolingLighting;

public sealed class LightingEngine : IDisposable
{
    private OpenRgbClient? _client;
    private readonly object _gate = new();
    private LightingLease? _legacyOwnership;
    private Process? _server;
    private DateTimeOffset _lastServerAttempt;

    private void Connect(bool startServer)
    {
        if (_client is not null) return;
        try { _client = new OpenRgbClient(name: "Cooling & Lighting Native", timeoutMs: 3000); }
        catch
        {
            if (startServer) StartInstalledServer();
            throw;
        }
    }

    private void StartInstalledServer()
    {
        if (DateTimeOffset.UtcNow - _lastServerAttempt < TimeSpan.FromSeconds(30)) return;
        _lastServerAttempt = DateTimeOffset.UtcNow;
        if (_server is {HasExited: false}) return;
        var running = Process.GetProcessesByName("OpenRGB");
        try { if (running.Length != 0) return; }
        finally { foreach (var process in running) process.Dispose(); }
        var candidates = new[] {Environment.GetEnvironmentVariable("OPENRGB_PATH"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "OpenRGB", "OpenRGB.exe"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "OpenRGB", "OpenRGB.exe")};
        var path = candidates.FirstOrDefault(p => p is not null && File.Exists(p));
        if (path is null) { Diagnostics.Log("OpenRGB server unavailable; set OPENRGB_PATH for a portable installation"); return; }
        _server?.Dispose();
        var launch = new ProcessStartInfo(path) {UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden};
        launch.ArgumentList.Add("--server"); launch.ArgumentList.Add("--server-host"); launch.ArgumentList.Add("127.0.0.1");
        _server = Process.Start(launch);
        Diagnostics.Log("Started installed OpenRGB local SDK backend");
    }

    private void AcquireOwnership()
    {
        if (_legacyOwnership is not null) return;
        var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "RGBTrayController");
        Directory.CreateDirectory(directory);
        var paths = new List<string> {Path.Combine(directory, "app.lock")};
        var packaged = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Packages", "OpenAI.Codex_2p2nqsd0c76g0", "LocalCache", "Local", "RGBTrayController", "app.lock");
        if (File.Exists(packaged)) paths.Add(packaged);
        try
        {
            _legacyOwnership = LightingLease.Acquire(paths);
        }
        catch (IOException)
        {
            throw new InvalidOperationException("The original RGB tray app still owns lighting. Native settings are saved; lighting takeover waits until it exits.");
        }
    }

    public string[] ReadDevices()
    {
        lock (_gate)
        {
            Connect(false);
            return _client!.GetAllControllerData().Select(d => d.Name).ToArray();
        }
    }

    public void Apply(NativeSettings settings)
    {
        lock (_gate)
        {
            try
            {
                AcquireOwnership();
                Connect(true);
                var devices = _client!.GetAllControllerData();
                if (devices.Length == 0) throw new InvalidOperationException("OpenRGB has not detected any devices yet");
                for (var i = 0; i < devices.Length; i++)
                {
                    var device = devices[i];
                    if (settings.ZoneSizes.TryGetValue(device.Name, out var zones))
                    {
                        for (var zone = 0; zone < device.Zones.Length; zone++)
                            if (zones.TryGetValue(device.Zones[zone].Name, out var size) && device.Zones[zone].LedCount != size)
                            {
                                if (size < device.Zones[zone].LedsMin || size > device.Zones[zone].LedsMax)
                                    throw new InvalidOperationException($"Saved LED count is outside the supported range for {device.Name} · {device.Zones[zone].Name}");
                                _client.ResizeZone(i, zone, size);
                            }
                        device = _client.GetControllerData(i);
                    }
                    var off = Array.FindIndex(device.Modes, m => m.Name.Equals("Off", StringComparison.OrdinalIgnoreCase));
                    if (!settings.LightingOn && off >= 0) { _client.UpdateMode(i, off); continue; }
                    _client.SetCustomMode(i);
                    device = _client.GetControllerData(i);
                    if (device.Leds.Length == 0) continue;
                    var color = settings.LightingOn ? new Color(settings.Color[0], settings.Color[1], settings.Color[2]) : new Color(0, 0, 0);
                    _client.UpdateLeds(i, Enumerable.Repeat(color, device.Leds.Length).ToArray());
                }
            }
            catch { _client?.Dispose(); _client = null; throw; }
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            _client?.Dispose(); _client = null;
            if (_server is {HasExited: false})
            {
                // Never stop a shared server; only the helper started by this instance.
                try { _server.Kill(); _server.WaitForExit(3000); }
                catch (Exception exception) when (exception is InvalidOperationException or System.ComponentModel.Win32Exception) { Diagnostics.Log("Cannot close owned RGB backend: " + exception.Message); }
            }
            _server?.Dispose(); _server = null;
            _legacyOwnership?.Dispose(); _legacyOwnership = null;
        }
    }
}
