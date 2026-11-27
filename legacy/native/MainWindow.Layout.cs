using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Shapes;
using C = System.Windows.Controls;
using M = System.Windows.Media;

namespace CoolingLighting;

public sealed partial class MainWindow
{
    private static SolidColorBrush Paint(string hex) => new((M.Color)M.ColorConverter.ConvertFromString(hex));
    private readonly C.ContentControl _pageHost = new();
    private readonly C.TextBlock _pageTitle = Label("Overview", 28, true);
    private readonly C.TextBlock _pageSubtitle = Label("Your cooling and lighting, in one place.", 13);
    private readonly C.TextBlock _cpuValue = Label("—", 36, true), _gpuValue = Label("—", 36, true), _fanValue = Label("—", 36, true);
    private readonly C.TextBlock _cpuDetail = Label("Waiting for sensors", 11), _gpuDetail = Label("Waiting for sensors", 11), _fanDetail = Label("Detected RPM channels", 11);
    private readonly C.WrapPanel _fanCards = new();
    private readonly C.WrapPanel _deviceCards = new();
    private readonly C.Canvas _cpuTrend = new() {Height = 40}, _gpuTrend = new() {Height = 40};
    private readonly Queue<double?> _cpuHistory = new(), _gpuHistory = new();
    private readonly C.TextBlock _lightHex = Label("#FFFFFF", 32, true), _lightState = Label("Saved colour", 12);
    private readonly C.Border _lightPreview = new() {CornerRadius = new CornerRadius(18), Width = 84, Height = 84};
    private C.Button? _lightToggle;
    private readonly List<C.Button> _navigation = [];
    private readonly List<FrameworkElement> _pages = [];
    private string[] _deviceNames = [];
    private int? _dragPoint;

    private static C.TextBlock Text(string text, double size = 13, string color = "#99A4B5", bool bold = false)
    {
        var label = Label(text, size, bold); label.Foreground = Paint(color); return label;
    }

    private static C.Border Card(UIElement content, Thickness? padding = null) => new()
    {
        Background = Paint("#1B1F27"), BorderBrush = Paint("#2C323D"), BorderThickness = new Thickness(1),
        CornerRadius = new CornerRadius(12), Padding = padding ?? new Thickness(22), Child = content,
        Margin = new Thickness(0, 0, 0, 18)
    };

    private static C.StackPanel Stack(params UIElement[] children)
    {
        var panel = new C.StackPanel(); foreach (var child in children) panel.Children.Add(child); return panel;
    }

    private static C.Grid Columns(double left = 1, double right = 1)
    {
        var grid = new C.Grid();
        grid.ColumnDefinitions.Add(new C.ColumnDefinition {Width = new GridLength(left, GridUnitType.Star)});
        grid.ColumnDefinitions.Add(new C.ColumnDefinition {Width = new GridLength(right, GridUnitType.Star)});
        return grid;
    }

    private C.Button Primary(string caption, Action action)
    {
        var button = Button(caption, action); button.Style = (Style)Resources["PrimaryButton"]; return button;
    }

    private void InitializeInterface()
    {
        Title = "Cooling & Lighting";
        Icon = System.Windows.Media.Imaging.BitmapFrame.Create(new Uri("pack://application:,,,/Assets/App.ico"));
        Width = 1240; Height = 840; MinWidth = 1040; MinHeight = 720;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        Background = Paint("#11141A"); Foreground = Paint("#EEF1F6");
        FontFamily = new M.FontFamily("Segoe UI"); FontSize = 13;
        Resources.MergedDictionaries.Add(new ResourceDictionary {Source = new Uri("/CoolingLighting;component/Theme.xaml", UriKind.Relative)});
        var shell = new C.Grid(); shell.ColumnDefinitions.Add(new C.ColumnDefinition {Width = new GridLength(210)});
        shell.ColumnDefinitions.Add(new C.ColumnDefinition()); Content = shell;
        var sidebar = new C.DockPanel {Margin = new Thickness(18, 26, 18, 20)};
        var sidebarBottom = Stack(Text("COOLING & LIGHTING", 10, "#687488", true), Text("Local control. No cloud.", 11),
            Button("Hide to tray", Hide), Text("Closing the window keeps\nmonitoring in the tray.", 11));
        C.DockPanel.SetDock(sidebarBottom, C.Dock.Bottom); sidebar.Children.Add(sidebarBottom);
        var brand = new C.StackPanel {Orientation = C.Orientation.Horizontal};
        brand.Children.Add(new C.Image {Source = new System.Windows.Media.Imaging.BitmapImage(new Uri("pack://application:,,,/Assets/App.png")), Width = 38, Height = 38, Margin = new Thickness(0, 0, 10, 0)});
        brand.Children.Add(Text("C / L", 26, "#B5EF79", true));
        var nav = Stack(brand, Text("COOLING & LIGHTING", 10, "#99A4B5", true));
        nav.Children.Add(new C.Border {Height = 40});
        var captions = new[] {"Overview", "Cooling", "Lighting", "Settings"};
        for (var i = 0; i < captions.Length; i++)
        {
            var index = i; var button = Button(captions[i], () => Navigate(index));
            button.HorizontalContentAlignment = System.Windows.HorizontalAlignment.Left; button.Padding = new Thickness(16, 13, 16, 13);
            button.Margin = new Thickness(0, 0, 0, 8); _navigation.Add(button); nav.Children.Add(button);
        }
        sidebar.Children.Add(nav);
        var sideBorder = new C.Border {Background = Paint("#15191F"), BorderBrush = Paint("#272D36"), BorderThickness = new Thickness(0, 0, 1, 0), Child = sidebar};
        shell.Children.Add(sideBorder);
        var main = new C.Grid {Margin = new Thickness(30, 24, 30, 16)}; C.Grid.SetColumn(main, 1); shell.Children.Add(main);
        main.RowDefinitions.Add(new C.RowDefinition {Height = GridLength.Auto});
        main.RowDefinitions.Add(new C.RowDefinition {Height = GridLength.Auto});
        main.RowDefinitions.Add(new C.RowDefinition());
        main.RowDefinitions.Add(new C.RowDefinition {Height = GridLength.Auto});
        var header = Columns(1, 1);
        _pageSubtitle.Foreground = Paint("#99A4B5"); header.Children.Add(Stack(_pageTitle, _pageSubtitle));
        _profile.Text = _settings.ActiveProfile ?? ""; _profile.ToolTip = "Choose a saved profile, or type a name to save your current settings."; _profile.MinWidth = 150; _profile.Width = 160; _profile.Height = 40; _profile.Margin = new Thickness(0, 0, 8, 0);
        var profiles = Row(); profiles.HorizontalAlignment = System.Windows.HorizontalAlignment.Right; profiles.VerticalAlignment = VerticalAlignment.Center;
        var profileBox = new C.Grid(); profileBox.Children.Add(_profile);
        var profileHint = Text("Profile name…", 12); profileHint.Margin = new Thickness(12, 0, 30, 0); profileHint.IsHitTestVisible = false;
        profileBox.Children.Add(profileHint); profiles.Children.Add(profileBox);
        var saveProfile = Button("Save", SaveNamedProfile); saveProfile.ToolTip = "Save current settings as a named profile"; saveProfile.Height = 40; profiles.Children.Add(saveProfile);
        var loadProfile = Button("Load", LoadProfile); loadProfile.ToolTip = "Apply a saved profile to cooling and lighting"; loadProfile.Height = 40; profiles.Children.Add(loadProfile);
        var profileLabel = Text("PROFILE", 10); profileLabel.Margin = new Thickness(0, 0, 8, 0); profiles.Children.Insert(0, profileLabel);
        void UpdateProfileActions()
        {
            var name = _profile.Text.Trim(); profileHint.Visibility = name.Length == 0 ? Visibility.Visible : Visibility.Hidden;
            saveProfile.IsEnabled = name.Length > 0; loadProfile.IsEnabled = ProfileStore.Names().Contains(name, StringComparer.OrdinalIgnoreCase);
        }
        _profile.AddHandler(C.TextBox.TextChangedEvent, new C.TextChangedEventHandler((_, _) => UpdateProfileActions()));
        _profile.SelectionChanged += (_, _) => Dispatcher.BeginInvoke(UpdateProfileActions);
        UpdateProfileActions();
        C.Grid.SetColumn(profiles, 1); header.Children.Add(profiles); main.Children.Add(header);
        _status.FontSize = 12; _status.Foreground = Paint("#D6DDBB"); _status.Margin = new Thickness(10, 6, 10, 6);
        var statusBar = new C.Border {Background = Paint("#222A20"), CornerRadius = new CornerRadius(6), Child = _status, Margin = new Thickness(0, 18, 0, 22)};
        C.Grid.SetRow(statusBar, 1); main.Children.Add(statusBar);
        C.Grid.SetRow(_pageHost, 2); main.Children.Add(_pageHost);
        _notice.FontSize = 11; _notice.Foreground = Paint("#A3AEBD"); _notice.Margin = new Thickness(0, 10, 0, 0);
        C.Grid.SetRow(_notice, 3); main.Children.Add(_notice);
        _pages.Add(OverviewPage()); _pages.Add(CoolingPage()); _pages.Add(LightingPage()); _pages.Add(SettingsPage());
        _graph.SizeChanged += (_, _) => DrawCurve();
        _points.TextChanged += (_, _) => DrawCurve();
        _output.SelectionChanged += (_, _) => LoadRule();
        foreach (var field in EditorFields()) field.TextChanged += (_, _) => MarkDraftChanged();
        _mode.SelectionChanged += (_, _) => MarkDraftChanged();
        _source.SelectionChanged += (_, _) => MarkDraftChanged();
        _rpm.SelectionChanged += (_, _) => MarkDraftChanged();
        _graph.MouseLeftButtonDown += BeginCurveDrag; _graph.MouseMove += MoveCurvePoint;
        _graph.MouseLeftButtonUp += (_, _) => { _dragPoint = null; _graph.ReleaseMouseCapture(); };
        Navigate(0); UpdateLightingPreview();
    }

    private void SaveNamedProfile()
    {
        RequireSavedDrafts();
        var name = _profile.Text;
        lock (_settingsGate)
        {
            var candidate = _settings with {ActiveProfile = name};
            ProfileStore.Save(name, candidate); SettingsStore.Save(candidate); _settings = candidate;
        }
        _profile.ItemsSource = ProfileStore.Names(); _profile.Text = name;
        _notice.Text = SettingsStore.LastSaveWarning ?? "Profile saved: " + name;
    }

    private void Navigate(int index)
    {
        var names = new[] {"Overview", "Cooling", "Lighting", "Settings"};
        var descriptions = new[] {"Temperatures, fan speeds and system health.", "Shape your fan response. Keep thermal protection.", "Set the mood across your connected devices.", "Startup, storage and system status."};
        _pageTitle.Text = names[index]; _pageSubtitle.Text = descriptions[index]; _pageHost.Content = _pages[index];
        for (var i = 0; i < _navigation.Count; i++)
        {
            _navigation[i].Background = Paint(i == index ? "#2B3B25" : "#15191F");
            _navigation[i].Foreground = Paint(i == index ? "#C2F598" : "#A3AEBD");
            _navigation[i].BorderBrush = Paint(i == index ? "#456138" : "#15191F");
        }
    }

    private FrameworkElement OverviewPage()
    {
        var page = new C.StackPanel();
        var metrics = new C.Primitives.UniformGrid {Columns = 3, Margin = new Thickness(0, 0, -12, 0)};
        foreach (var (caption, value, detail, trend) in new[] {
            ("CPU TEMPERATURE", _cpuValue, _cpuDetail, _cpuTrend), ("GPU TEMPERATURE", _gpuValue, _gpuDetail, _gpuTrend),
            ("FAN CHANNELS", _fanValue, _fanDetail, new C.Canvas {Height = 40})})
        {
            detail.Foreground = Paint("#99A4B5");
            var card = Card(Stack(Text(caption, 10, "#B1BACA", true), value, detail, trend)); card.Margin = new Thickness(0, 0, 12, 20); metrics.Children.Add(card);
        }
        page.Children.Add(metrics);
        var fanHeading = new C.DockPanel(); var configure = Button("Configure fans →", () => Navigate(1));
        C.DockPanel.SetDock(configure, C.Dock.Right); fanHeading.Children.Add(configure); fanHeading.Children.Add(Label("Fan channels", 18, true)); page.Children.Add(fanHeading);
        page.Children.Add(Text("Live readings from detected outputs. Control ownership is shown above.", 12));
        page.Children.Add(_fanCards);
        foreach (var (caption, binding, width) in new[] {("DEVICE", "Hardware", 2d), ("SENSOR", "Sensor", 1.7d), ("READING", "Reading", 1d)})
            _sensors.Columns.Add(new C.DataGridTextColumn {Header = caption, Binding = new System.Windows.Data.Binding(binding), Width = new C.DataGridLength(width, C.DataGridLengthUnitType.Star)});
        _sensors.MinHeight = 160; _sensors.MaxHeight = 280;
        page.Children.Add(Card(new C.Expander {Header = "Sensor library · view all readings", Foreground = Paint("#DCE3EE"),
            Content = Stack(Text("Actual hardware values. Unavailable sensors are never shown as 0°C.", 12), _sensors)}));
        return new C.ScrollViewer {Content = page};
    }

    private FrameworkElement CoolingPage()
    {
        var grid = Columns(0.85, 1.3);
        _output.MinWidth = 0; _source.MinWidth = 0; _source.Height = 145; _rpm.MinWidth = 0;
        _source.SetValue(C.ScrollViewer.HorizontalScrollBarVisibilityProperty, C.ScrollBarVisibility.Auto);
        var setup = Stack(Label("Fan setup", 18, true), Text("Select an output, then build its response.", 12),
            Field("FAN OUTPUT", _output), Field("DISPLAY NAME", _name), Field("CONTROL MODE", _mode),
            Field("TEMPERATURE SOURCES", _source), Text("Click to select multiple. The hottest reading wins.", 11), Field("RPM FEEDBACK", _rpm));
        var settings = new C.Primitives.UniformGrid {Columns = 2};
        foreach (var (caption, field) in new[] {("Minimum %", _minimum), ("Manual %", _manual), ("Startup %", _start), ("Failsafe °C", _critical), ("Hysteresis °C", _hysteresis), ("Ramp down %/s", _ramp)})
        { var form = Field(caption, field); form.Margin = new Thickness(0, 0, 10, 10); settings.Children.Add(form); }
        setup.Children.Add(new C.Expander {Header = "Safety & smoothing", Foreground = Paint("#C6CEDB"), Content = settings, Margin = new Thickness(0, 14, 0, 10)});
        var left = Card(setup); left.Margin = new Thickness(0, 0, 18, 0); grid.Children.Add(left);
        _graph.MinWidth = 280; _graph.Height = 210; _graph.Background = Paint("#151A21");
        var presetRow = Row();
        foreach (var (name, points) in new[] {("Quiet", "30:30, 50:35, 70:65, 85:100"), ("Balanced", "30:40, 50:55, 70:80, 85:100"), ("Performance", "30:50, 50:65, 70:90, 85:100")})
            presetRow.Children.Add(Button(name, () => _points.Text = points));
        var curve = Stack(Label("Temperature response", 18, true), Text("Drag a point to tune the curve. Changes apply only when saved.", 12), presetRow,
            new C.Border {Height = 14}, _graph, new C.Border {Height = 12}, Field("CURVE POINTS · °C : %", _points),
            Text("Use increasing temperatures and non-decreasing fan speeds.", 11));
        var actions = Row(); actions.Margin = new Thickness(0, 18, 0, 8);
        actions.Children.Add(Primary("Save fan rule", SaveRule)); actions.Children.Add(Button("Reset draft", ResetDraft)); curve.Children.Add(actions);
        curve.Children.Add(_draftStatus);
        curve.Children.Add(Text("PROTECTED BY DESIGN", 10, "#B5EF79", true));
        curve.Children.Add(Text("Missing temperatures or RPM feedback request full cooling. Firmware mode leaves fan speeds to BIOS/GPU control. Only one app can control an output at a time.", 12));
        var right = Card(curve); right.Margin = new Thickness(0); C.Grid.SetColumn(right, 1); grid.Children.Add(right);
        return new C.ScrollViewer {Content = grid};
    }

    private static C.StackPanel Field(string caption, FrameworkElement control)
    {
        control.Margin = new Thickness(0, 4, 0, 0); control.HorizontalAlignment = System.Windows.HorizontalAlignment.Stretch;
        if (control is C.TextBox or C.ComboBox) control.Height = 40;
        return new C.StackPanel {Children = {Text(caption, 10, "#A7B1C0", true), control}, Margin = new Thickness(0, 8, 0, 8)};
    }

    private FrameworkElement LightingPage()
    {
        var page = new C.StackPanel();
        var preview = Columns(0.7, 2); preview.Children.Add(_lightPreview);
        _lightToggle = Primary("Turn lights off", () => SetLighting(_settings.Color, !_settings.LightingOn));
        _lightToggle.HorizontalAlignment = System.Windows.HorizontalAlignment.Left;
        var detail = Stack(Text("GLOBAL COLOUR", 10, "#A7B1C0", true), _lightHex, _lightState, _lightToggle);
        C.Grid.SetColumn(detail, 1); preview.Children.Add(detail); page.Children.Add(Card(preview));
        var swatches = new C.WrapPanel();
        foreach (var (name, color) in new (string, byte[])[] {("White", [255,255,255]), ("Ocean", [0,80,255]), ("Ice", [0,220,255]), ("Mint", [0,255,130]), ("Violet", [140,0,255]), ("Ember", [255,55,25])})
        {
            var chip = new C.StackPanel {Orientation = C.Orientation.Horizontal};
            chip.Children.Add(new Ellipse {Width = 15, Height = 15, Fill = new SolidColorBrush(M.Color.FromRgb(color[0], color[1], color[2])), Margin = new Thickness(0, 0, 10, 0)});
            chip.Children.Add(new C.TextBlock {Text = name});
            var button = Button(name, () => SetLighting(color, true)); button.Content = chip; button.Margin = new Thickness(0, 0, 10, 10); swatches.Children.Add(button);
        }
        swatches.Children.Add(Button("Custom colour…", CustomColor));
        page.Children.Add(Card(Stack(Label("Colour palette", 18, true), Text("One colour across your controllable devices. Saved immediately.", 12), new C.Border {Height = 12}, swatches)));
        _rgbStatus.Foreground = Paint("#B6C1D1"); _rgbStatus.FontSize = 12;
        page.Children.Add(Label("Devices", 18, true)); page.Children.Add(_rgbStatus); page.Children.Add(_deviceCards);
        page.Children.Add(Text("Only software-controllable lights can be changed. Standalone fan and pump lighting may use hardware controls.", 11));
        return new C.ScrollViewer {Content = page};
    }

    private FrameworkElement SettingsPage()
    {
        var page = new C.StackPanel();
        page.Children.Add(Card(Stack(Label("System status", 18, true), Text(_engine.Administrator
            ? "Hardware access enabled · CPU, motherboard and GPU monitoring are available."
            : "Limited hardware access. Close this window and open the app from its desktop shortcut to restore full access.", 13),
            Text("Curve and manual modes use thermal safeguards. Firmware mode leaves cooling protection to BIOS/GPU control.", 12))));
        page.Children.Add(Card(Stack(Label("Saved settings", 18, true), Text("Lighting saves immediately. Fan rules and named profiles save when you press Save. A recovery copy protects against interrupted saves.", 13),
            Text(SettingsStore.DirectoryPath, 11, "#B5EF79"))));
        page.Children.Add(Card(Stack(Label("Windows startup", 18, true), Text(StartupPlan.InstalledForThisExecutable()
            ? "Enabled · starts in the tray with administrator hardware access, 10 seconds after sign-in. The desktop shortcut opens this same instance."
            : "Windows startup is not configured for this copy of the app. Use the installed desktop shortcut.", 13))));
        page.Children.Add(Card(Stack(Label("Tray behaviour", 18, true), Text("Closing the window keeps cooling and lighting running. Use the tray icon to open the dashboard again.", 13))));
        page.Children.Add(Button("Quit Cooling & Lighting", RequestQuit));
        page.Children.Add(Text("Quitting releases any controlled fans to BIOS/GPU control.", 11));
        return new C.ScrollViewer {Content = page};
    }

    private void RefreshOverview(IReadOnlyDictionary<string, SensorReading> readings)
    {
        var cpu = TemperatureSelection.Cpu(readings.Values);
        var gpu = TemperatureSelection.Gpu(readings.Values);
        _cpuValue.Text = cpu?.Value is { } c ? $"{c:0}°" : "—";
        _gpuValue.Text = gpu?.Value is { } g ? $"{g:0}°" : "—";
        _cpuDetail.Text = cpu?.Value is null ? "CPU temperature unavailable" : cpu.Hardware;
        _gpuDetail.Text = gpu?.Hardware ?? "No GPU temperature available";
        var fans = readings.Values.Where(s => s.Type == "Fan" &&
            (_settings.Fans.Any(r => r.RpmSensorId == s.Id) || s.Value > 0))
            .Concat(_settings.Fans.Where(r => !string.IsNullOrWhiteSpace(r.RpmSensorId) && !readings.ContainsKey(r.RpmSensorId))
                .Select(r => new SensorReading(r.RpmSensorId!, "Saved channel · sensor unavailable", r.Name, "Fan", null, DateTimeOffset.UtcNow, false)))
            .DistinctBy(s => s.Id).ToArray();
        _fanValue.Text = fans.Length.ToString(CultureInfo.InvariantCulture);
        _fanDetail.Text = fans.Length == 1 ? "1 configured or active channel" : $"{fans.Length} configured or active channels";
        AddTrend(_cpuHistory, cpu?.Value, _cpuTrend); AddTrend(_gpuHistory, gpu?.Value, _gpuTrend);
        _fanCards.Children.Clear();
        foreach (var fan in fans)
        {
            var rule = _settings.Fans.FirstOrDefault(f => f.RpmSensorId == fan.Id);
            var content = Stack(Text(rule?.Name ?? fan.Name, 12, "#DCE3EE", true),
                Label(fan.Value is { } rpm ? $"{rpm:0} rpm" : "Unavailable", 25, true), Text(fan.Hardware, 11),
                Text(rule?.Mode == "Firmware" || rule is null ? "BIOS / GPU controlled" : rule.Mode, 10, "#B5EF79"));
            var card = Card(content, new Thickness(16)); card.Width = 245; card.Margin = new Thickness(0, 10, 12, 20); _fanCards.Children.Add(card);
        }
    }

    private static void AddTrend(Queue<double?> history, double? value, C.Canvas canvas)
    {
        history.Enqueue(value); while (history.Count > 90) history.Dequeue(); canvas.Children.Clear();
        var values = history.ToArray(); var width = Math.Max(100, canvas.ActualWidth); Polyline? line = null;
        for (var i = 0; i < values.Length; i++)
        {
            if (values[i] is not { } sample || !double.IsFinite(sample)) {line = null; continue;}
            if (line is null) {line = new Polyline {Stroke = Paint("#B5EF79"), StrokeThickness = 1.6}; canvas.Children.Add(line);}
            line.Points.Add(new System.Windows.Point(i / 89d * width, 39 - Math.Clamp(sample, 0, 110) / 110 * 36));
        }
    }

    private void UpdateLightingPreview()
    {
        _lightHex.Text = "#" + Convert.ToHexString(_settings.Color);
        _lightState.Text = _settings.LightingOn ? "ON · saved preference" : "OFF · colour remembered";
        _lightState.Foreground = Paint(_settings.LightingOn ? "#B5EF79" : "#99A4B5");
        _lightPreview.Background = _settings.LightingOn ? new SolidColorBrush(M.Color.FromRgb(_settings.Color[0], _settings.Color[1], _settings.Color[2])) : Paint("#303640");
        if (_lightToggle is not null) _lightToggle.Content = _settings.LightingOn ? "Turn lights off" : "Turn lights on";
    }

    private void RefreshDeviceCards(string[] names)
    {
        if (_deviceNames.SequenceEqual(names) && _deviceCards.Children.Count > 0) return;
        _deviceNames = names; _deviceCards.Children.Clear();
        foreach (var name in names)
        {
            var zones = _settings.ZoneSizes.GetValueOrDefault(name);
            var description = zones is null ? "Detected by OpenRGB" : string.Join(" · ", zones.Select(z => $"{z.Key}: {z.Value} LEDs"));
            var card = Card(Stack(Text("DETECTED", 10, "#B5EF79", true), Label(name, 15, true), Text(description, 11)), new Thickness(18));
            card.Width = 280; card.Margin = new Thickness(0, 12, 12, 0); _deviceCards.Children.Add(card);
        }
        if (names.Length == 0) _deviceCards.Children.Add(Text("Waiting for the local RGB backend…", 13));
    }

    private void BeginCurveDrag(object sender, MouseButtonEventArgs args)
    {
        CurvePoint[] points; try {points = ParsePoints();} catch {return;}
        var position = args.GetPosition(_graph);
        for (var i = 0; i < points.Length; i++)
        {
            var mapped = CurvePosition(points[i]);
            if ((mapped - position).Length > 16) continue;
            _dragPoint = i; _graph.CaptureMouse(); args.Handled = true; break;
        }
    }

    private System.Windows.Point CurvePosition(CurvePoint point) => new(46 + point.Temperature / 110 * Math.Max(100, _graph.ActualWidth - 68),
        20 + (100 - point.Percent) / 100 * (_graph.Height - 56));

    private void MoveCurvePoint(object sender, System.Windows.Input.MouseEventArgs args)
    {
        if (_dragPoint is not { } index || args.LeftButton != MouseButtonState.Pressed) return;
        var points = ParsePoints(); if (index >= points.Length) return;
        var p = args.GetPosition(_graph);
        var temperature = Math.Round((p.X - 46) / Math.Max(100, _graph.ActualWidth - 68) * 110);
        var percent = Math.Round(100 - (p.Y - 20) / (_graph.Height - 56) * 100);
        try {points = CurveEditor.MovePoint(points, index, temperature, percent);}
        catch (ArgumentException) {_dragPoint = null; _graph.ReleaseMouseCapture(); return;}
        _points.Text = string.Join(", ", points.Select(point => $"{point.Temperature.ToString(CultureInfo.InvariantCulture)}:{point.Percent.ToString(CultureInfo.InvariantCulture)}"));
    }
}
