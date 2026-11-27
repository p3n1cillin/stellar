using System.IO;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Xml.Linq;

namespace CoolingLighting;

public static class StartupPlan
{
    public const string TaskName = "CoolingLighting-Native";
    private static readonly XNamespace Schema = "http://schemas.microsoft.com/windows/2004/02/mit/task";

    public static string CreateXml(string executable, string userSid)
    {
        if (!Path.IsPathFullyQualified(executable) || !Path.GetFileName(executable).Equals("CoolingLighting.exe", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Startup must target an absolute CoolingLighting.exe path, not a shell or script");
        _ = new SecurityIdentifier(userSid);
        XElement Element(string name, object value) => new(Schema + name, value);
        var task = new XElement(Schema + "Task", new XAttribute("version", "1.2"),
            new XElement(Schema + "RegistrationInfo", Element("Description", "Cooling & Lighting native application; one interactive tray instance at sign-in.")),
            new XElement(Schema + "Triggers", new XElement(Schema + "LogonTrigger", Element("Enabled", "true"), Element("UserId", userSid), Element("Delay", "PT10S"))),
            new XElement(Schema + "Principals", new XElement(Schema + "Principal", new XAttribute("id", "CurrentUser"),
                Element("UserId", userSid), Element("LogonType", "InteractiveToken"), Element("RunLevel", "HighestAvailable"))),
            new XElement(Schema + "Settings", Element("MultipleInstancesPolicy", "IgnoreNew"),
                Element("DisallowStartIfOnBatteries", "false"), Element("StopIfGoingOnBatteries", "false"),
                Element("AllowHardTerminate", "false"), Element("StartWhenAvailable", "true"),
                Element("AllowStartOnDemand", "true"), Element("Enabled", "true"),
                Element("ExecutionTimeLimit", "PT0S"),
                new XElement(Schema + "RestartOnFailure", Element("Interval", "PT1M"), Element("Count", "3"))),
            new XElement(Schema + "Actions", new XAttribute("Context", "CurrentUser"), new XElement(Schema + "Exec",
                Element("Command", executable), Element("Arguments", "--tray"), Element("WorkingDirectory", Path.GetDirectoryName(executable)!))));
        return new XDocument(task).ToString();
    }

    // Microsoft TASK_VALIDATE_ONLY = 1: checks schema without registering or running a task.
    // This intentionally offers no installer or enablement path before hardware cutover.
    public static void ValidateWithWindows(string xml)
    {
        object? service = null, folder = null, result = null;
        try
        {
            var type = Type.GetTypeFromProgID("Schedule.Service") ?? throw new InvalidOperationException("Windows Task Scheduler is unavailable");
            service = Activator.CreateInstance(type)!;
            ((dynamic)service).Connect();
            folder = ((dynamic)service).GetFolder("\\");
            result = ((dynamic)folder).RegisterTask(TaskName, xml, 1, null, null, 3, null);
        }
        finally
        {
            foreach (var item in new[] {result, folder, service})
                if (item is not null && Marshal.IsComObject(item)) Marshal.FinalReleaseComObject(item);
        }
    }

    public static string CurrentXml() => CreateXml(Path.Combine(AppContext.BaseDirectory, "CoolingLighting.exe"),
        WindowsIdentity.GetCurrent().User?.Value ?? throw new InvalidOperationException("User identity unavailable"));

    public static void LaunchRegisteredTask()
    {
        object? service = null, folder = null, task = null, running = null;
        try
        {
            service = Activator.CreateInstance(Type.GetTypeFromProgID("Schedule.Service")!)!;
            ((dynamic)service).Connect();
            folder = ((dynamic)service).GetFolder("\\");
            task = ((dynamic)folder).GetTask(TaskName);
            running = ((dynamic)task).Run(null);
        }
        finally
        {
            foreach (var item in new[] {running, task, folder, service})
                if (item is not null && Marshal.IsComObject(item)) Marshal.FinalReleaseComObject(item);
        }
    }

    public static bool InstalledForThisExecutable()
    {
        object? service = null, folder = null, task = null;
        try
        {
            service = Activator.CreateInstance(Type.GetTypeFromProgID("Schedule.Service")!)!;
            ((dynamic)service).Connect(); folder = ((dynamic)service).GetFolder("\\");
            task = ((dynamic)folder).GetTask(TaskName);
            var document = XDocument.Parse((string)((dynamic)task).Xml);
            return (bool)((dynamic)task).Enabled && document.Descendants(Schema + "Command").Single().Value.Equals(
                Path.Combine(AppContext.BaseDirectory, "CoolingLighting.exe"), StringComparison.OrdinalIgnoreCase);
        }
        catch { return false; }
        finally
        {
            foreach (var item in new[] {task, folder, service})
                if (item is not null && Marshal.IsComObject(item)) Marshal.FinalReleaseComObject(item);
        }
    }
}
