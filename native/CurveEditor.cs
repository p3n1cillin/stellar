namespace CoolingLighting;

public static class CurveEditor
{
    public static CurvePoint[] MovePoint(CurvePoint[] points, int index, double temperature, double percent)
    {
        new FanRule {ControlId = "editor", Points = points}.Validate();
        if (index < 0 || index >= points.Length || !double.IsFinite(temperature) || !double.IsFinite(percent))
            throw new ArgumentException("Invalid curve edit");
        var updated = points.ToArray();
        var minimumTemperature = index == 0 ? 0 : Math.BitIncrement(points[index - 1].Temperature);
        var maximumTemperature = index == points.Length - 1 ? 110 : Math.BitDecrement(points[index + 1].Temperature);
        temperature = Math.Clamp(Math.Round(temperature, 1), minimumTemperature, maximumTemperature);
        percent = Math.Clamp(Math.Round(percent), index == 0 ? 0 : points[index - 1].Percent, index == points.Length - 1 ? 100 : points[index + 1].Percent);
        updated[index] = new(temperature, percent);
        return updated;
    }

    public static void Test()
    {
        var points = new[] {new CurvePoint(30, 30), new CurvePoint(50, 60), new CurvePoint(85, 100)};
        var count = 0;
        void Check(bool valid, string name) {if (!valid) throw new Exception("FAIL: " + name); count++;}
        Check(MovePoint(points, 0, -10, -10)[0] == new CurvePoint(0, 0), "curve drag clamps lower bounds");
        Check(MovePoint(points, 2, 200, 200)[2] == new CurvePoint(110, 100), "curve drag clamps upper bounds");
        var middle = MovePoint(points, 1, 100, 5);
        Check(middle[1].Temperature < 85 && middle[1].Temperature > 30 && middle[1].Percent == 30, "curve drag preserves monotonic shape");
        Check(points[1] == new CurvePoint(50, 60), "curve drag leaves saved curve untouched");
        var narrow = MovePoint([new(30, 30), new(30.01, 40), new(30.02, 50)], 1, 100, 50);
        Check(narrow[1].Temperature > 30 && narrow[1].Temperature < 30.02, "curve drag handles closely spaced points");
        try {MovePoint(points, 5, 50, 50); throw new Exception("Invalid point accepted");} catch (ArgumentException) {count++;}
        Console.WriteLine($"PASS: {count} visual curve-editor checks");
    }
}
