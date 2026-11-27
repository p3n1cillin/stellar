using System.IO;
using System.Text.Json;

namespace CoolingLighting;

public sealed record NativeSettings
{
    public int Version { get; init; } = 1;
    public bool LightingOn { get; init; } = true;
    public string? ActiveProfile { get; init; }
    public byte[] Color { get; init; } = [255, 255, 255];
    public Dictionary<string, Dictionary<string, int>> ZoneSizes { get; init; } = [];
    public List<FanRule> Fans { get; init; } = [];
}

public static class SettingsStore
{
    // Programs is excluded from packaged-host AppData redirection. Both a build
    // launched by the desktop coding host and the non-packaged scheduled task
    // must read the same files; otherwise startup silently sees another profile.
    public static string DirectoryPath => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "CoolingLighting", "Data");
    public static string ConfigPath => Path.Combine(DirectoryPath, "settings.json");
    private static readonly JsonSerializerOptions Options = new() { WriteIndented = true };
    private static readonly object Gate = new();
    public static string? LastLoadWarning { get; private set; }
    public static string? LastSaveWarning { get; private set; }

    public static NativeSettings Load()
    {
        return LoadFrom(ConfigPath, Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "RGBTrayController", "config.json"));
    }

    public static NativeSettings LoadFrom(string configPath, string? legacyPath = null)
    {
        LastLoadWarning = null;
        foreach (var path in new[] { configPath, configPath + ".bak" })
        {
            try
            {
                var settings = JsonSerializer.Deserialize<NativeSettings>(File.ReadAllText(path)) ?? throw new InvalidDataException("Empty settings");
                Validate(settings);
                if (path.EndsWith(".bak", StringComparison.Ordinal)) LastLoadWarning = "Recovered settings from the backup copy";
                return settings;
            }
            catch (Exception exception) when (exception is IOException or JsonException or ArgumentException or InvalidDataException or UnauthorizedAccessException) { }
        }
        if (File.Exists(configPath) || File.Exists(configPath + ".bak"))
        {
            LastLoadWarning = "Settings could not be read. Cooling stays in firmware mode; damaged files were left untouched.";
            return new NativeSettings();
        }
        if (legacyPath is null) return new NativeSettings();
        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(legacyPath));
            var root = document.RootElement;
            var imported = new NativeSettings
            {
                LightingOn = root.GetProperty("on").GetBoolean(),
                Color = root.GetProperty("color").EnumerateArray().Select(c => c.GetByte()).ToArray(),
                ZoneSizes = JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, int>>>(root.GetProperty("zone_sizes").GetRawText()) ?? []
            };
            Validate(imported);
            SaveTo(configPath, imported);
            return imported;
        }
        catch (Exception exception) when (exception is IOException or JsonException or ArgumentException or InvalidOperationException or KeyNotFoundException or InvalidDataException or UnauthorizedAccessException or OverflowException) { }
        return new NativeSettings();
    }

    public static void Validate(NativeSettings settings)
    {
        if (settings.Version != 1 || settings.Color is null || settings.Color.Length != 3 || settings.Fans is null || settings.ZoneSizes is null || settings.Fans.Any(r => r is null))
            throw new InvalidDataException("Invalid settings");
        foreach (var rule in settings.Fans) rule.Validate();
        if (settings.ActiveProfile is not null) ProfileStore.ValidateName(settings.ActiveProfile);
        if (settings.Fans.Select(r => r.ControlId).Distinct().Count() != settings.Fans.Count)
            throw new InvalidDataException("Duplicate fan rules");
        foreach (var zones in settings.ZoneSizes.Values)
            if (zones is null || zones.Values.Any(v => v < 0 || v > 10000)) throw new InvalidDataException("Invalid LED counts");
    }

    public static void Save(NativeSettings settings)
        => SaveTo(ConfigPath, settings);

    public static void SaveTo(string configPath, NativeSettings settings)
    {
        Validate(settings);
        lock (Gate)
        {
            LastSaveWarning = null;
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(configPath))!);
            var payload = JsonSerializer.SerializeToUtf8Bytes(settings, Options);
            AtomicWrite(configPath, payload);
            // The primary commit succeeded. A backup failure must not report that
            // the save failed while the next startup reads the newly committed state.
            try { AtomicWrite(configPath + ".bak", payload); }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
            { LastSaveWarning = "Settings saved, but the recovery copy could not be updated: " + exception.Message; }
        }
    }

    public static void AtomicWrite(string path, byte[] payload)
    {
        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            using (var file = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            { file.Write(payload); file.Flush(true); }
            File.Move(temporary, path, true);
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }
}
