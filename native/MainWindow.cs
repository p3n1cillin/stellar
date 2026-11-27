using System.Globalization;
using System.Windows;
using System.Windows.Media;
using System.Windows.Shapes;
using C = System.Windows.Controls;
using M = System.Windows.Media;

namespace CoolingLighting;

public sealed partial class MainWindow : Window
{
    private readonly HardwareEngine _engine = new();
    private readonly LightingEngine _lights = new();
    private readonly CancellationTokenSource _stop = new();
    private readonly object _settingsGate = new();
    private NativeSettings _settings = SettingsStore.Load();
    private IReadOnlyDictionary<string, SensorReading> _readings = new Dictionary<string, SensorReading>();
    private readonly C.TextBlock _status = Label("Opening hardware…");
    private readonly C.TextBlock _rgbStatus = Label("Connecting to lighting…");
    private readonly C.TextBlock _notice = Label("Lighting saves automatically · fan edits apply when saved.", 12);
    private readonly C.TextBlock _draftStatus = Label("No unsaved fan changes", 11);
    private readonly Dictionary<string, EditorDraft> _drafts = [];
    private string? _loadedControlId;
    private bool _loadingEditor, _refreshingChoices, _editorDirty;
    private record EditorDraft(string[] Fields, string Mode, string[] Sources, string? Rpm);
    private readonly C.DataGrid _sensors = new() { IsReadOnly = true, AutoGenerateColumns = false, MinHeight = 130, MaxHeight = 200 };
    private readonly C.ComboBox _output = new() { MinWidth = 320, DisplayMemberPath = "Label" };
    private readonly C.ListBox _source = new() { MinWidth = 440, Height = 100, DisplayMemberPath = "Label", SelectionMode = C.SelectionMode.Multiple };
    private readonly C.ComboBox _rpm = new() { MinWidth = 320, DisplayMemberPath = "Label" };
    private readonly C.ComboBox _profile = new() { MinWidth = 200, IsEditable = true, ItemsSource = ProfileStore.Names(), Text = "" };
    private readonly C.ComboBox _mode = new() { ItemsSource = new[] {"Firmware", "Curve", "Manual"}, SelectedIndex = 0, MinWidth = 130 };
    private readonly C.TextBox _name = Input("Fan"), _minimum = Input("30"), _manual = Input("70"), _critical = Input("85");
    private readonly C.TextBox _points = Input("35:40, 55:55, 70:80, 85:100");
    private readonly C.TextBox _start = Input("40"), _hysteresis = Input("2"), _ramp = Input("5");
    private readonly C.Canvas _graph = new() { Height = 180, MinWidth = 450, Background = new M.SolidColorBrush(M.Color.FromRgb(22, 32, 48)), ClipToBounds = true };
    private Task? _worker;
    private Task? _rgbWorker;
    private bool _closing;
    private bool _closedSafely;
    private int _lightingRevision;
    private readonly SemaphoreSlim _lightingGate = new(1, 1);
    private readonly System.Windows.Forms.NotifyIcon _tray = new();
    private System.Drawing.Icon? _appIcon;
    private bool _quitRequested;

    private record Choice(string Id, string Label)
    {
        public override string ToString() => Label;
    }
    private record DisplaySensor(string Hardware, string Sensor, string Reading, string Id);

    public MainWindow(bool startInTray = false)
    {
        InitializeInterface();
        var trayMenu = new System.Windows.Forms.ContextMenuStrip();
        trayMenu.Items.Add("Open Cooling & Lighting", null, (_, _) => Dispatcher.Invoke(ShowDashboard));
        trayMenu.Items.Add("Toggle lighting", null, (_, _) => Dispatcher.Invoke(() => SetLighting(_settings.Color, !_settings.LightingOn)));
        trayMenu.Items.Add(new System.Windows.Forms.ToolStripSeparator());
        trayMenu.Items.Add("Quit — return fans to firmware", null, (_, _) => Dispatcher.Invoke(RequestQuit));
        using (var iconResource = System.Windows.Application.GetResourceStream(new Uri("pack://application:,,,/Assets/App.ico")).Stream)
            _appIcon = new System.Drawing.Icon(iconResource, 32, 32);
        _tray.Icon = _appIcon;
        _tray.Text = "Cooling & Lighting";
        _tray.ContextMenuStrip = trayMenu;
        _tray.DoubleClick += (_, _) => Dispatcher.Invoke(ShowDashboard);
        _tray.Visible = true;
        Loaded += (_, _) =>
        {
            DrawCurve(); _worker = Task.Run(Monitor); _rgbWorker = RestoreLighting();
            if (SettingsStore.LastLoadWarning is { } warning) _notice.Text = warning;
            if (startInTray) Hide();
        };
        Microsoft.Win32.SystemEvents.PowerModeChanged += PowerChanged;
        Microsoft.Win32.SystemEvents.SessionEnding += SessionEnding;
        Closing += CloseSafely;
    }

    private void PowerChanged(object sender, Microsoft.Win32.PowerModeChangedEventArgs args)
    {
        if (args.Mode == Microsoft.Win32.PowerModes.Suspend) _engine.Suspend(true);
        else if (args.Mode == Microsoft.Win32.PowerModes.Resume) _engine.Suspend(false);
    }

    private void SessionEnding(object sender, Microsoft.Win32.SessionEndingEventArgs args)
    {
        _stop.Cancel();
        // Windows logoff/shutdown can terminate the UI before an awaited task resumes.
        // Release synchronously here, with writes blocked inside the hardware engine.
        _engine.Suspend(true);
        Diagnostics.Log("Windows session ending; firmware handoff requested");
    }

    private async void CloseSafely(object? sender, System.ComponentModel.CancelEventArgs args)
    {
        if (_closedSafely) return;
        args.Cancel = true;
        if (!_quitRequested) { Hide(); return; }
        if (_closing) return;
        _closing = true;
        IsEnabled = false;
        _status.Text = "Releasing cooling to firmware and closing connections…";
        _stop.Cancel();
        Microsoft.Win32.SystemEvents.PowerModeChanged -= PowerChanged;
        Microsoft.Win32.SystemEvents.SessionEnding -= SessionEnding;
        try
        {
            if (_worker is not null) await _worker;
            if (_rgbWorker is not null) await _rgbWorker;
            await _lightingGate.WaitAsync();
            try { await Task.Run(_lights.Dispose); } finally { _lightingGate.Release(); }
        }
        finally { Diagnostics.Log("Native app closed"); _tray.Dispose(); _appIcon?.Dispose(); _closedSafely = true; Close(); }
    }

    public void ShowDashboard() { Show(); WindowState = WindowState.Normal; Activate(); }
    public void RequestQuit()
    {
        if ((_editorDirty || _drafts.Count > 0) && System.Windows.MessageBox.Show(this,
            "Unsaved fan drafts will be discarded. Quit without saving?", "Unsaved fan changes",
            MessageBoxButton.YesNo, MessageBoxImage.Warning, MessageBoxResult.No) != MessageBoxResult.Yes) return;
        _quitRequested = true; Close();
    }

    private static C.TextBlock Label(string text, double size = 14, bool bold = false) => new()
    { Text = text, Foreground = M.Brushes.White, FontSize = size, FontWeight = bold ? FontWeights.SemiBold : FontWeights.Normal,
      Margin = new Thickness(0, 5, 0, 5), TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
    private static C.TextBox Input(string text) => new() { Text = text, VerticalContentAlignment = VerticalAlignment.Center };
    private static C.WrapPanel Row() => new() { Margin = new Thickness(0, 4, 0, 4) };
    private static C.Button Button(string text, Action action)
    {
        var button = new C.Button { Content = text, Margin = new Thickness(0, 0, 8, 0) };
        button.Click += (_, _) => { try { action(); } catch (Exception exception) { System.Windows.MessageBox.Show(
            exception is FormatException or OverflowException ? "Enter valid numbers in the fan settings. Curve points use temperature:speed pairs, for example 35:40, 55:55, 85:100." : exception.Message,
            "Cooling & Lighting", MessageBoxButton.OK, MessageBoxImage.Warning); } };
        return button;
    }

    private async Task Monitor()
    {
        try
        {
            Diagnostics.Log("Native engine opening");
            _engine.Open();
            string? previousStatus = null;
            var nextSnapshot = DateTimeOffset.MinValue;
            while (!_stop.IsCancellationRequested)
            {
                var readings = _engine.Read();
                _readings = readings;
                NativeSettings settings; lock (_settingsGate) settings = _settings;
                string status;
                try
                {
                    var decisions = _engine.Apply(settings.Fans, readings);
                    var other = HardwareEngine.FanControlRunning();
                    status = other ? "Cooling paused · another fan controller is running" :
                        _engine.Administrator ? "Cooling & lighting ready" : "Limited hardware access · open the app using its desktop shortcut";
                    if (decisions.Any(d => d.Value.Emergency)) status += " · Thermal failsafe: full cooling";
                }
                catch (Exception exception) { status = exception.Message; }
                if (status != previousStatus) { Diagnostics.Log(status); previousStatus = status; }
                if (DateTimeOffset.UtcNow >= nextSnapshot)
                {
                    Diagnostics.Snapshot(_engine.Administrator, readings, _engine.ErrorSnapshot(), status);
                    nextSnapshot = DateTimeOffset.UtcNow.AddSeconds(10);
                }
                var errors = _engine.ErrorSnapshot();
                if (!_closing) await Dispatcher.InvokeAsync(() => Refresh(readings, status, errors));
                await Task.Delay(1000, _stop.Token);
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception exception)
        {
            Diagnostics.Log("Hardware error: " + exception);
            if (!_closing) await Dispatcher.InvokeAsync(() => _status.Text = "Hardware error: " + exception.Message);
        }
        finally { _engine.Dispose(); }
    }

    private void Refresh(IReadOnlyDictionary<string, SensorReading> readings, string status, string[] errors)
    {
        RefreshOverview(readings);
        _status.Text = status.Replace("Cooling & lighting ready", "Cooling ready") + (errors.Length > 0 ? " · " + string.Join("; ", errors) : "");
        _status.Foreground = Paint(errors.Length > 0 || status.Contains("paused") || status.Contains("Limited") || status.Contains("failsafe") ? "#FFD48A" : "#D6DDBB");
        _sensors.ItemsSource = readings.Values.Where(s => s.Type is "Temperature" or "Fan" or "Control")
            .Select(s => new DisplaySensor(s.Hardware, s.Name,
                s.Value is null ? "Unavailable" : $"{s.Value:0.0}{(s.Type == "Temperature" ? " °C" : s.Type == "Fan" ? " RPM" : " %")}", s.Id)).ToArray();
        _refreshingChoices = true;
        try
        {
        var controls = readings.Values.Where(s => s.CanControl).Select(s => new Choice(s.Id, s.Hardware + " · " + s.Name))
            .Concat(_settings.Fans.Where(f => !readings.ContainsKey(f.ControlId)).Select(f => new Choice(f.ControlId, "Missing saved output: " + f.Name))).ToArray();
        if ((_output.ItemsSource as Choice[])?.Select(c => c.Id).SequenceEqual(controls.Select(c => c.Id)) != true)
        { var id = (_output.SelectedItem as Choice)?.Id; _output.ItemsSource = controls; _output.SelectedItem = controls.FirstOrDefault(c => c.Id == id) ?? controls.FirstOrDefault(); }
        var currentRule = _settings.Fans.FirstOrDefault(f => f.ControlId == (_output.SelectedItem as Choice)?.Id);
        var selectedSources = _source.SelectedItems.Cast<Choice>().Select(s => s.Id).ToArray();
        var retainedSources = (currentRule?.TemperatureSensorIds ?? []).Concat(_editorDirty ? selectedSources : []).Distinct();
        var sources = readings.Values.Where(s => s.Type == "Temperature")
            .Select(s => new Choice(s.Id, s.Hardware + " · " + s.Name + (s.Value is null ? " (unavailable)" : "")))
            .Concat(retainedSources.Where(id => !readings.ContainsKey(id)).Select(id => new Choice(id, "Missing saved sensor: " + id))).ToArray();
        if ((_source.ItemsSource as Choice[])?.Select(c => c.Id).SequenceEqual(sources.Select(c => c.Id)) != true)
        {
            var selected = _source.SelectedItems.Cast<Choice>().Select(c => c.Id).ToArray();
            _source.ItemsSource = sources;
            SelectSources(sources, selected.Length > 0 || _editorDirty ? selected : currentRule?.TemperatureSensorIds ?? [], currentRule is null && !_editorDirty);
        }
        var retainedRpm = _editorDirty ? (_rpm.SelectedItem as Choice)?.Id : currentRule?.RpmSensorId;
        var rpm = new[] { new Choice("", "No RPM pairing") }.Concat(readings.Values.Where(s => s.Type == "Fan")
            .Select(s => new Choice(s.Id, s.Hardware + " · " + s.Name)))
            .Concat(!string.IsNullOrWhiteSpace(retainedRpm) && !readings.ContainsKey(retainedRpm)
                ? new[] {new Choice(retainedRpm, "Missing saved RPM sensor: " + retainedRpm)} : []).ToArray();
        if ((_rpm.ItemsSource as Choice[])?.Select(c => c.Id).SequenceEqual(rpm.Select(c => c.Id)) != true)
        {
            var rule = _settings.Fans.FirstOrDefault(f => f.ControlId == (_output.SelectedItem as Choice)?.Id);
            var id = (_rpm.SelectedItem as Choice)?.Id ?? rule?.RpmSensorId;
            _rpm.ItemsSource = rpm; _rpm.SelectedItem = rpm.FirstOrDefault(c => c.Id == id) ?? PreferredRpm(rpm);
        }
        }
        finally { _refreshingChoices = false; }
    }

    private Choice? PreferredSource(Choice[] sources)
    {
        var controlId = (_output.SelectedItem as Choice)?.Id;
        var selected = TemperatureSelection.ForOutput(_readings.Values, controlId is null ? null : _readings.GetValueOrDefault(controlId));
        return sources.FirstOrDefault(s => s.Id == selected?.Id);
    }

    private void SelectSources(Choice[] sources, string[] ids, bool allowDefault = false)
    {
        _source.SelectedItems.Clear();
        foreach (var source in sources.Where(s => ids.Contains(s.Id))) _source.SelectedItems.Add(source);
        if (allowDefault && ids.Length == 0 && PreferredSource(sources) is { } preferred) _source.SelectedItems.Add(preferred);
    }

    private Choice? PreferredRpm(Choice[] sources)
    {
        var id = (_output.SelectedItem as Choice)?.Id;
        var hardware = id is null ? null : _readings.GetValueOrDefault(id)?.Hardware;
        var choices = sources.Where(s => _readings.GetValueOrDefault(s.Id)?.Hardware == hardware).ToArray();
        return choices.FirstOrDefault(s => s.Id.Split('/').Last() == id?.Split('/').Last()) ?? sources.FirstOrDefault();
    }

    private void LoadRule()
    {
        if (_output.SelectedItem is not Choice control) return;
        if (_editorDirty && _loadedControlId is { } oldId) _drafts[oldId] = CaptureDraft();
        _loadingEditor = true;
        try
        {
        var rule = _settings.Fans.FirstOrDefault(f => f.ControlId == control.Id) ?? new FanRule {ControlId = control.Id, Name = control.Label};
        // Preserve missing saved references in the editor instead of silently replacing them.
        if (_source.ItemsSource is Choice[] existingSources)
            _source.ItemsSource = existingSources.Concat(rule.TemperatureSensorIds.Where(id => !existingSources.Any(s => s.Id == id))
                .Select(id => new Choice(id, "Missing saved sensor: " + id))).ToArray();
        if (_rpm.ItemsSource is Choice[] existingRpm && !string.IsNullOrWhiteSpace(rule.RpmSensorId) && !existingRpm.Any(s => s.Id == rule.RpmSensorId))
            _rpm.ItemsSource = existingRpm.Append(new Choice(rule.RpmSensorId, "Missing saved RPM sensor: " + rule.RpmSensorId)).ToArray();
        _name.Text = rule.Name; _mode.SelectedItem = rule.Mode;
        _minimum.Text = rule.MinimumPercent.ToString(CultureInfo.InvariantCulture);
        _manual.Text = rule.ManualPercent.ToString(CultureInfo.InvariantCulture);
        _critical.Text = rule.CriticalTemperature.ToString(CultureInfo.InvariantCulture);
        _start.Text = rule.StartPercent.ToString(CultureInfo.InvariantCulture);
        _hysteresis.Text = rule.Hysteresis.ToString(CultureInfo.InvariantCulture);
        _ramp.Text = rule.RampDownPerSecond.ToString(CultureInfo.InvariantCulture);
        _points.Text = string.Join(", ", rule.Points.Select(p => $"{p.Temperature.ToString(CultureInfo.InvariantCulture)}:{p.Percent.ToString(CultureInfo.InvariantCulture)}"));
        if (_source.ItemsSource is Choice[] sources) SelectSources(sources, rule.TemperatureSensorIds, !_settings.Fans.Any(f => f.ControlId == control.Id));
        if (_rpm.ItemsSource is Choice[] rpm) _rpm.SelectedItem = rpm.FirstOrDefault(s => s.Id == rule.RpmSensorId) ?? PreferredRpm(rpm);
        _loadedControlId = control.Id;
        _editorDirty = _drafts.TryGetValue(control.Id, out var draft);
        if (draft is not null) RestoreDraft(draft);
        DrawCurve();
        }
        finally { _loadingEditor = false; UpdateDraftStatus(); }
    }

    private C.TextBox[] EditorFields() => [_name, _minimum, _manual, _critical, _start, _hysteresis, _ramp, _points];
    private EditorDraft CaptureDraft() => new(EditorFields().Select(f => f.Text).ToArray(), (string)_mode.SelectedItem,
        _source.SelectedItems.Cast<Choice>().Select(s => s.Id).ToArray(), (_rpm.SelectedItem as Choice)?.Id);
    private void RestoreDraft(EditorDraft draft)
    {
        var fields = EditorFields(); for (var i = 0; i < fields.Length; i++) fields[i].Text = draft.Fields[i];
        _mode.SelectedItem = draft.Mode;
        if (_source.ItemsSource is Choice[] sources)
        {
            sources = sources.Concat(draft.Sources.Where(id => !sources.Any(s => s.Id == id)).Select(id => new Choice(id, "Missing draft sensor: " + id))).ToArray();
            _source.ItemsSource = sources;
            _source.SelectedItems.Clear();
            foreach (var source in sources.Where(s => draft.Sources.Contains(s.Id))) _source.SelectedItems.Add(source);
        }
        if (_rpm.ItemsSource is Choice[] rpm)
        {
            if (draft.Rpm is {Length: > 0} id && !rpm.Any(s => s.Id == id)) { rpm = rpm.Append(new Choice(id, "Missing draft RPM sensor: " + id)).ToArray(); _rpm.ItemsSource = rpm; }
            _rpm.SelectedItem = rpm.FirstOrDefault(s => s.Id == draft.Rpm);
        }
    }
    private void MarkDraftChanged()
    {
        if (_loadingEditor || _refreshingChoices || _loadedControlId is null) return;
        _editorDirty = true; UpdateDraftStatus();
    }
    private void UpdateDraftStatus()
    {
        _draftStatus.Text = _editorDirty ? "Unsaved draft · kept while switching fans; save before quitting" : "No unsaved fan changes";
        _draftStatus.Foreground = Paint(_editorDirty ? "#FFD48A" : "#99A4B5");
    }
    private void ResetDraft()
    {
        if (_loadedControlId is { } id) _drafts.Remove(id);
        _editorDirty = false; LoadRule();
    }
    private void RequireSavedDrafts()
    {
        if (_editorDirty || _drafts.Count > 0) throw new ArgumentException("Save or reset your unsaved fan drafts before saving or loading a profile.");
    }

    private CurvePoint[] ParsePoints() => _points.Text.Split(',', StringSplitOptions.RemoveEmptyEntries).Select(value =>
    {
        var pair = value.Split(':');
        if (pair.Length != 2) throw new ArgumentException("Use temperature:speed pairs, for example 35:40, 55:55, 85:100");
        return new CurvePoint(double.Parse(pair[0], CultureInfo.InvariantCulture), double.Parse(pair[1], CultureInfo.InvariantCulture));
    }).ToArray();

    private void SaveRule()
    {
        if (_output.SelectedItem is not Choice control) throw new ArgumentException("Select a fan output");
        var rule = new FanRule {ControlId = control.Id, Name = _name.Text, Mode = (string)_mode.SelectedItem,
            Calibration = _settings.Fans.FirstOrDefault(f => f.ControlId == control.Id)?.Calibration ?? [],
            RpmSensorId = (_rpm.SelectedItem as Choice)?.Id,
            TemperatureSensorIds = _source.SelectedItems.Cast<Choice>().Select(s => s.Id).ToArray(),
            MinimumPercent = double.Parse(_minimum.Text, CultureInfo.InvariantCulture),
            ManualPercent = double.Parse(_manual.Text, CultureInfo.InvariantCulture),
            StartPercent = double.Parse(_start.Text, CultureInfo.InvariantCulture),
            Hysteresis = double.Parse(_hysteresis.Text, CultureInfo.InvariantCulture),
            RampDownPerSecond = double.Parse(_ramp.Text, CultureInfo.InvariantCulture),
            CriticalTemperature = double.Parse(_critical.Text, CultureInfo.InvariantCulture), Points = ParsePoints()};
        rule.Validate();
        lock (_settingsGate)
        {
            var fans = _settings.Fans.Where(f => f.ControlId != control.Id).Append(rule).ToList();
            var candidate = _settings with {Fans = fans};
            SettingsStore.Save(candidate); _settings = candidate;
        }
        _notice.Text = SettingsStore.LastSaveWarning ?? "Fan rule saved. The engine will apply it only with exclusive hardware access.";
        _drafts.Remove(control.Id); _editorDirty = false; UpdateDraftStatus();
        DrawCurve();
    }

    private void LoadProfile()
    {
        RequireSavedDrafts();
        var candidate = ProfileStore.Load(_profile.Text) with {ActiveProfile = _profile.Text};
        lock (_settingsGate) { SettingsStore.Save(candidate); _settings = candidate; }
        LoadRule();
        SetLighting(candidate.Color, candidate.LightingOn);
        _notice.Text = SettingsStore.LastSaveWarning ?? "Profile loaded: " + _profile.Text;
    }

    private void DrawCurve()
    {
        _graph.Children.Clear();
        CurvePoint[] points; try { points = ParsePoints(); } catch { return; }
        if (points.Length < 2 || points.Any(p => !double.IsFinite(p.Temperature) || !double.IsFinite(p.Percent))) return;
        var width = Math.Max(100, _graph.ActualWidth - 68);
        var height = _graph.Height - 56;
        for (var i = 0; i <= 4; i++)
        {
            var y = 20 + i * height / 4;
            _graph.Children.Add(new Line {X1 = 46, X2 = 46 + width, Y1 = y, Y2 = y, Stroke = Paint("#303743"), StrokeThickness = 1});
            var label = Text($"{100 - i * 25}%", 10);
            C.Canvas.SetLeft(label, 3); C.Canvas.SetTop(label, y - 11); _graph.Children.Add(label);
        }
        foreach (var temperature in new[] {0, 25, 50, 75, 100})
        {
            var x = 46 + temperature / 110d * width;
            var label = Text($"{temperature}°", 10);
            C.Canvas.SetLeft(label, x - 8); C.Canvas.SetTop(label, 20 + height + 9); _graph.Children.Add(label);
        }
        var line = new Polyline {Stroke = Paint("#B5EF79"), StrokeThickness = 2.5, IsHitTestVisible = false};
        var fill = new Polygon {Fill = new SolidColorBrush(M.Color.FromArgb(24, 181, 239, 121)), IsHitTestVisible = false};
        fill.Points.Add(new System.Windows.Point(46, 20 + height));
        foreach (var point in points) {var position = CurvePosition(point); line.Points.Add(position); fill.Points.Add(position);}
        fill.Points.Add(new System.Windows.Point(CurvePosition(points[^1]).X, 20 + height));
        _graph.Children.Add(fill); _graph.Children.Add(line);
        foreach (var point in points)
        {
            var position = CurvePosition(point);
            var dot = new Ellipse {Width = 11, Height = 11, Fill = Paint("#151A21"), Stroke = Paint("#B5EF79"), StrokeThickness = 2.5,
                Cursor = System.Windows.Input.Cursors.Hand, ToolTip = $"{point.Temperature:0}°C · {point.Percent:0}%"};
            C.Canvas.SetLeft(dot, position.X - 5.5); C.Canvas.SetTop(dot, position.Y - 5.5); _graph.Children.Add(dot);
        }
    }

    private async void SetLighting(byte[] color, bool on)
    {
        if (_closing) return;
        NativeSettings candidate;
        try
        {
            lock (_settingsGate)
            {
                candidate = _settings with {Color = color, LightingOn = on};
                SettingsStore.Save(candidate); _settings = candidate;
            }
            _notice.Text = SettingsStore.LastSaveWarning ?? "Lighting settings saved to disk.";
        }
        catch (Exception exception) { _notice.Text = "Lighting change not saved or applied: " + exception.Message; Diagnostics.Log(_notice.Text); return; }
        ++_lightingRevision;
        UpdateLightingPreview();
        _rgbStatus.Text = "Saved · applying lighting…";
        await ApplyLighting(candidate, _lightingRevision);
    }

    private async Task RestoreLighting()
    {
        try
        {
            while (!_stop.IsCancellationRequested)
            {
                NativeSettings candidate; lock (_settingsGate) candidate = _settings;
                await ApplyLighting(candidate, _lightingRevision);
                try { RefreshDeviceCards(await Task.Run(_lights.ReadDevices)); }
                catch { RefreshDeviceCards([]); }
                await Task.Delay(5000, _stop.Token);
            }
        }
        catch (OperationCanceledException) { }
    }

    private async Task ApplyLighting(NativeSettings candidate, int revision)
    {
        await _lightingGate.WaitAsync();
        try
        {
            if (_closing || revision != _lightingRevision) return;
            await Task.Run(() => _lights.Apply(candidate));
            if (!_closing && revision == _lightingRevision) _rgbStatus.Text = $"{(candidate.LightingOn ? "On" : "Off")} · #{Convert.ToHexString(candidate.Color)} · applied by native RGB engine";
        }
        catch (Exception exception) { if (!_closing) _rgbStatus.Text = "Lighting saved; connection error: " + exception.Message; }
        finally { _lightingGate.Release(); }
    }

    private void CustomColor()
    {
        using var picker = new System.Windows.Forms.ColorDialog {Color = System.Drawing.Color.FromArgb(_settings.Color[0], _settings.Color[1], _settings.Color[2]), FullOpen = true};
        if (picker.ShowDialog() == System.Windows.Forms.DialogResult.OK) SetLighting([picker.Color.R, picker.Color.G, picker.Color.B], true);
    }

}
