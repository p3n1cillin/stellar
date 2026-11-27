using System.IO;
using System.Text;
using System.Text.Json;

namespace CoolingLighting;

public static class Diagnostics
{
    private static readonly object Gate = new();
    public static void Log(string message)
    {
        try
        {
            lock (Gate)
            {
                Directory.CreateDirectory(SettingsStore.DirectoryPath);
                var path = Path.Combine(SettingsStore.DirectoryPath, "app.log");
                if (File.Exists(path) && new FileInfo(path).Length > 2_000_000) File.Move(path, path + ".previous", true);
                File.AppendAllText(path, $"{DateTimeOffset.Now:O} {message}{Environment.NewLine}", Encoding.UTF8);
            }
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { }
    }

    public static void Snapshot(bool administrator, IReadOnlyDictionary<string, SensorReading> readings, string[] errors, string status)
    {
        try
        {
            lock (Gate)
            {
                Directory.CreateDirectory(SettingsStore.DirectoryPath);
                SettingsStore.AtomicWrite(Path.Combine(SettingsStore.DirectoryPath, "hardware-status.json"),
                    JsonSerializer.SerializeToUtf8Bytes(new { Timestamp = DateTimeOffset.UtcNow, Administrator = administrator,
                        Status = status, Sensors = readings.Values, Errors = errors }, new JsonSerializerOptions { WriteIndented = true }));
            }
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { Log("Cannot save diagnostic snapshot: " + exception.Message); }
    }
}
