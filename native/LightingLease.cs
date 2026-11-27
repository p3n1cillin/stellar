using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace CoolingLighting;

public sealed class LightingLease : IDisposable
{
    private readonly List<FileStream> _files = [];
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandle(SafeFileHandle file, StringBuilder path, uint size, uint flags);

    public static LightingLease Acquire(IEnumerable<string> paths)
    {
        var lease = new LightingLease();
        var actualPaths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        try
        {
            foreach (var path in paths)
            {
                var file = new FileStream(path, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.ReadWrite);
                try
                {
                    var finalPath = new StringBuilder(32768);
                    var length = GetFinalPathNameByHandle(file.SafeFileHandle, finalPath, (uint)finalPath.Capacity, 0);
                    if (length == 0 || length >= finalPath.Capacity) throw new IOException("Cannot identify lighting lock file");
                    // Packaged-host redirection can make two different paths refer
                    // to one file. Lock that file once, not against ourselves.
                    if (!actualPaths.Add(finalPath.ToString())) { file.Dispose(); continue; }
                    file.Lock(0, 1);
                    if (file.Length == 0) { file.WriteByte(0); file.Flush(true); }
                    lease._files.Add(file);
                }
                catch { file.Dispose(); throw; }
            }
            return lease;
        }
        catch { lease.Dispose(); throw; }
    }

    public void Dispose() { foreach (var file in _files) file.Dispose(); _files.Clear(); }

    public static void Test()
    {
        var directory = Path.Combine(Path.GetTempPath(), "CoolingLighting-lock-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var first = Path.Combine(directory, "one.lock"); var second = Path.Combine(directory, "two.lock");
        var count = 0;
        void Check(bool value) { if (!value) throw new Exception("Lighting lease test failed"); count++; }
        try
        {
            using (Acquire([first, Path.Combine(directory, ".", "one.lock")])) { Check(File.Exists(first)); }
            using (Acquire([first, second])) { Check(File.Exists(second)); }
            using (var held = Acquire([second]))
            {
                try { using var blocked = Acquire([first, second]); throw new Exception("Competing lease allowed"); }
                catch (IOException) { count++; }
                using (Acquire([first])) { Check(true); } // failed acquisition releases earlier locks
            }
            using (Acquire([first, second])) { Check(true); } // normal exit releases everything
            var lease = Acquire([first]); lease.Dispose(); lease.Dispose(); Check(true);
            Console.WriteLine($"PASS: {count} lighting ownership checks (temporary files only)");
        }
        finally { foreach (var file in Directory.GetFiles(directory)) File.Delete(file); Directory.Delete(directory); }
    }
}
