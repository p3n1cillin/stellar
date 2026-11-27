using System.Xml.Linq;

namespace CoolingLighting;

public static class StartupTests
{
    public static void Run()
    {
        var count = 0;
        void Check(bool condition, string name) { if (!condition) throw new Exception("FAIL: " + name); count++; }
        const string path = "C:\\Example & Test\\CoolingLighting.exe";
        const string sid = "S-1-5-21-100-200-300-1001";
        var xml = XDocument.Parse(StartupPlan.CreateXml(path, sid));
        XNamespace ns = "http://schemas.microsoft.com/windows/2004/02/mit/task";
        string Value(string name) => xml.Descendants(ns + name).First().Value;
        Check(Value("Command") == path, "startup XML escapes spaces and ampersands without changing path");
        Check(Value("Arguments") == "--tray", "startup goes directly to tray");
        Check(Value("WorkingDirectory") == "C:\\Example & Test", "startup uses executable directory");
        Check(Value("LogonType") == "InteractiveToken", "startup uses logged-in user's session, not stored password");
        Check(Value("RunLevel") == "HighestAvailable", "startup requests required hardware privileges");
        Check(xml.Descendants(ns + "UserId").All(n => n.Value == sid), "trigger and principal limited to same user");
        Check(Value("MultipleInstancesPolicy") == "IgnoreNew", "startup avoids duplicate tasks");
        Check(Value("ExecutionTimeLimit") == "PT0S", "cooling daemon has no finite task timeout");
        Check(Value("AllowHardTerminate") == "false", "scheduler does not hard-terminate owned fan controls");
        Check(Value("Interval") == "PT1M" && Value("Count") == "3", "bounded restart on application failure");
        try { StartupPlan.CreateXml("CoolingLighting.exe", sid); throw new Exception("Relative startup accepted"); }
        catch (ArgumentException) { count++; }
        try { StartupPlan.CreateXml("C:\\Windows\\System32\\cmd.exe", sid); throw new Exception("Shell startup accepted"); }
        catch (ArgumentException) { count++; }
        Console.WriteLine($"PASS: {count} startup-plan checks (no task registration)");
    }
}
