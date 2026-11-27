using System.IO;

namespace CoolingLighting;

public static class ProfileStore
{
    private static string DirectoryPath => Path.Combine(SettingsStore.DirectoryPath, "profiles");
    public static string[] Names() => NamesIn(DirectoryPath);
    internal static string[] NamesIn(string directory) => Directory.Exists(directory)
        ? Directory.GetFiles(directory, "*.json").Select(Path.GetFileNameWithoutExtension).OfType<string>().Order().ToArray() : [];

    internal static void ValidateName(string name)
    {
        if (string.IsNullOrWhiteSpace(name) || name.Length > 60 || name != name.Trim()
            || name.Any(c => !(char.IsLetterOrDigit(c) || c is ' ' or '-' or '_')))
            throw new ArgumentException("Use a profile name of 1–60 letters, numbers, spaces, hyphens or underscores");
        var reserved = new[] {"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"};
        if (reserved.Contains(name, StringComparer.OrdinalIgnoreCase)) throw new ArgumentException("Reserved profile name");
    }

    private static string PathFor(string directory, string name)
    { ValidateName(name); return Path.Combine(directory, name + ".json"); }

    public static void Save(string name, NativeSettings settings) => SaveIn(DirectoryPath, name, settings);
    internal static void SaveIn(string directory, string name, NativeSettings settings)
        => SettingsStore.SaveTo(PathFor(directory, name), settings with {ActiveProfile = name});

    public static NativeSettings Load(string name)
        => LoadFrom(DirectoryPath, name);

    internal static NativeSettings LoadFrom(string directory, string name)
    {
        var path = PathFor(directory, name);
        // Do not silently activate defaults when both profile copies are corrupt.
        foreach (var candidate in new[] { path, path + ".bak" })
        {
            try
            {
                var settings = System.Text.Json.JsonSerializer.Deserialize<NativeSettings>(File.ReadAllText(candidate));
                if (settings is null) continue;
                SettingsStore.Validate(settings);
                return settings;
            }
            catch (Exception exception) when (exception is IOException or System.Text.Json.JsonException or ArgumentException or InvalidDataException or UnauthorizedAccessException) { }
        }
        throw new InvalidDataException("This profile cannot be read; the current settings have not changed");
    }
}
