using System.Text.Json;

namespace CoolingLighting;

public static class Program
{
    [STAThread]
    public static void Main(string[] args)
    {
        try { Run(args); }
        catch (Exception exception) { Diagnostics.Log("Startup failure: " + exception); Environment.ExitCode = 1; }
    }

    private static void Run(string[] args)
    {
        Diagnostics.Log("Launch " + string.Join(" ", args) + " from " + AppContext.BaseDirectory);
        if (args.Contains("--launch-startup"))
        {
            try
            {
                StartupPlan.LaunchRegisteredTask();
                var opened = false;
                for (var i = 0; i < 20; i++)
                {
                    try
                    {
                        using var ready = EventWaitHandle.OpenExisting("Local\\CoolingLighting.Ready");
                        if (!ready.WaitOne(500)) continue;
                        using var show = EventWaitHandle.OpenExisting("Local\\CoolingLighting.Show"); show.Set(); opened = true; break;
                    }
                    catch (WaitHandleCannotBeOpenedException) { Thread.Sleep(500); }
                }
                if (!opened) throw new TimeoutException("The app did not become ready. Try the shortcut again; diagnostic details are in the app's Data folder.");
            }
            catch (Exception exception) { Environment.ExitCode = 1; Diagnostics.Log("Desktop launch failed: " + exception); System.Windows.MessageBox.Show("Cannot launch the installed startup task: " + exception.Message, "Cooling & Lighting"); }
            return;
        }
        if (args.Contains("--quit"))
        {
            try { using var quit = EventWaitHandle.OpenExisting("Local\\CoolingLighting.Quit"); quit.Set(); }
            catch (WaitHandleCannotBeOpenedException) { }
            return;
        }
        if (args.Contains("--handover-check"))
        {
            try { HandoverCheck.Run(); }
            catch (Exception exception) { Diagnostics.Log("Handover check: " + exception); Environment.ExitCode = 1; }
            return;
        }
        if (args.Contains("--startup-export"))
        {
            SettingsStore.AtomicWrite(System.IO.Path.Combine(SettingsStore.DirectoryPath, "startup-plan.xml"), System.Text.Encoding.UTF8.GetBytes(StartupPlan.CurrentXml()));
            return;
        }
        if (args.Contains("--self-test")) { PolicyTests.Run(); SettingsTests.Run(); OutputTests.Run(); StartupTests.Run(); CurveEditor.Test(); LightingLease.Test(); BrandingTests.Run(); TemperatureSelection.Test(); return; }
        if (args.Contains("--startup-check"))
        {
            StartupPlan.ValidateWithWindows(StartupPlan.CurrentXml());
            Console.WriteLine("PASS: Windows Task Scheduler validated the startup plan. No task was registered or launched.");
            return;
        }
        if (args.Contains("--rgb-probe"))
        {
            using var lights = new LightingEngine();
            Console.WriteLine(JsonSerializer.Serialize(lights.ReadDevices()));
            return;
        }
        if (args.Contains("--probe"))
        {
            using var engine = new HardwareEngine();
            engine.Open(); engine.Read(); Thread.Sleep(1200);
            Console.WriteLine(JsonSerializer.Serialize(new { Timestamp = DateTimeOffset.UtcNow,
                Administrator = engine.Administrator, Sensors = engine.Read().Values.ToArray(),
                Errors = engine.Errors.ToArray() }, new JsonSerializerOptions { WriteIndented = true }));
            return;
        }
        if (args.Contains("--import-probe"))
        {
            using var engine = new HardwareEngine(); engine.Open();
            var rules = FanControlImport.Parse(System.IO.File.ReadAllBytes(FanControlImport.DefaultPath), engine.Read());
            Console.WriteLine(JsonSerializer.Serialize(rules, new JsonSerializerOptions { WriteIndented = true }));
            return;
        }
        using var mutex = new Mutex(false, "Local\\CoolingLighting.Native");
        var first = false;
        try { first = mutex.WaitOne(args.Contains("--elevated") ? 30000 : 0); }
        catch (AbandonedMutexException) { first = true; }
        using var showWindow = new EventWaitHandle(false, EventResetMode.AutoReset, "Local\\CoolingLighting.Show");
        if (!first) { showWindow.Set(); return; }
        using var readyWindow = new EventWaitHandle(false, EventResetMode.ManualReset, "Local\\CoolingLighting.Ready");
        readyWindow.Reset();
        var application = new System.Windows.Application { ShutdownMode = System.Windows.ShutdownMode.OnExplicitShutdown };
        application.DispatcherUnhandledException += (_, e) =>
        {
            Diagnostics.Log("UI error: " + e.Exception);
            System.Windows.MessageBox.Show(e.Exception.Message, "Cooling & Lighting");
            e.Handled = true;
        };
        var window = new MainWindow(args.Contains("--tray"));
        // MainWindow's Loaded handler hides tray launches before desktop activation is permitted.
        window.Loaded += (_, _) => readyWindow.Set();
        using var quitWindow = new EventWaitHandle(false, EventResetMode.AutoReset, "Local\\CoolingLighting.Quit");
        window.Closed += (_, _) => application.Shutdown();
        var registration = ThreadPool.RegisterWaitForSingleObject(showWindow,
            (_, _) => application.Dispatcher.BeginInvoke(window.ShowDashboard), null, Timeout.Infinite, false);
        var quitRegistration = ThreadPool.RegisterWaitForSingleObject(quitWindow,
            (_, _) => application.Dispatcher.BeginInvoke(window.RequestQuit), null, Timeout.Infinite, false);
        try { application.Run(window); }
        finally { registration.Unregister(null); quitRegistration.Unregister(null); mutex.ReleaseMutex(); }
    }
}
