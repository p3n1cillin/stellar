using System.IO;
using System.Resources;
using System.Windows.Media.Imaging;

namespace CoolingLighting;

public static class BrandingTests
{
    public static void Run()
    {
        var count = 0;
        void Check(bool condition, string name) { if (!condition) throw new Exception("FAIL: " + name); count++; }
        using var resources = new ResourceReader(typeof(Program).Assembly.GetManifestResourceStream("CoolingLighting.g.resources")!);
        var assets = new Dictionary<string, byte[]>();
        foreach (System.Collections.DictionaryEntry item in resources)
        {
            if (item.Key is not string key || !key.StartsWith("assets/")) continue;
            using var copy = new MemoryStream(); ((Stream)item.Value!).CopyTo(copy); assets[key] = copy.ToArray();
        }
        Check(assets.ContainsKey("assets/app.ico"), "Windows icon embedded in WPF resources");
        Check(assets.ContainsKey("assets/app.png"), "sharp sidebar branding embedded in WPF resources");
        using var iconStream = new MemoryStream(assets["assets/app.ico"]);
        using var iconReader = new BinaryReader(iconStream, System.Text.Encoding.UTF8, true);
        Check(iconReader.ReadUInt16() == 0 && iconReader.ReadUInt16() == 1 && iconReader.ReadUInt16() == 7, "multi-resolution Windows icon header");
        var widths = new List<int>();
        for (var i = 0; i < 7; i++) { var width = iconReader.ReadByte(); widths.Add(width == 0 ? 256 : width); iconStream.Seek(15, SeekOrigin.Current); }
        Check(widths.SequenceEqual(new[] {16,24,32,48,64,128,256}), "icon covers tray through high-DPI desktop sizes");
        iconStream.Position = 0;
        using var trayIcon = new System.Drawing.Icon(iconStream, 32, 32);
        Check(trayIcon.Width == 32 && trayIcon.Height == 32, "tray icon loads without generic icon fallback");
        using var image = new MemoryStream(assets["assets/app.png"]);
        var decoder = BitmapDecoder.Create(image, BitmapCreateOptions.None, BitmapCacheOption.OnLoad);
        Check(decoder.Frames[0].PixelWidth == 256 && decoder.Frames[0].PixelHeight == 256, "sidebar image decodes at full resolution");
        Console.WriteLine($"PASS: {count} branding resource checks");
    }
}
