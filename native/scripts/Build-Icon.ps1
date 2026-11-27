# Deterministic code-native branding. Produce Windows multi-resolution ICO assets.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$iconOutputDirectory = Join-Path (Split-Path $PSScriptRoot) 'Assets'
$iconMaster = New-Object Drawing.Bitmap 256,256
$iconGraphics = [Drawing.Graphics]::FromImage($iconMaster)
$iconGraphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
$iconGraphics.Clear([Drawing.Color]::Transparent)
$iconBackground = New-Object Drawing.Drawing2D.GraphicsPath
foreach ($corner in @(@(10,10,180),@(142,10,270),@(142,142,0),@(10,142,90))) { $iconBackground.AddArc($corner[0],$corner[1],104,104,$corner[2],90) }
$iconBackground.CloseFigure()
$iconGradient = New-Object Drawing.Drawing2D.LinearGradientBrush ([Drawing.Rectangle]::new(10,10,236,236)),([Drawing.ColorTranslator]::FromHtml('#1D2431')),([Drawing.ColorTranslator]::FromHtml('#11141A')),45
$iconGraphics.FillPath($iconGradient,$iconBackground)
$iconBorder = New-Object Drawing.Pen ([Drawing.ColorTranslator]::FromHtml('#303A49')),3
$iconGraphics.DrawPath($iconBorder,$iconBackground)
$iconArc = New-Object Drawing.Pen ([Drawing.ColorTranslator]::FromHtml('#61DCE8')),8
$iconArc.StartCap = $iconArc.EndCap = [Drawing.Drawing2D.LineCap]::Round
$iconGraphics.DrawArc($iconArc,38,38,180,180,-45,90)
$iconBlade = New-Object Drawing.Drawing2D.GraphicsPath
$iconBlade.AddBezier(0,-18,30,-25,60,-58,29,-77)
$iconBlade.AddBezier(29,-77,-20,-86,-44,-57,-24,-23)
$iconBlade.AddBezier(-24,-23,-16,-16,-8,-16,0,-18)
$iconBlade.CloseFigure()
$iconLime = New-Object Drawing.SolidBrush ([Drawing.ColorTranslator]::FromHtml('#B5EF79'))
foreach ($angle in @(0,90,180,270)) {
    $bladeCopy = $iconBlade.Clone()
    $bladeMatrix = New-Object Drawing.Drawing2D.Matrix
    $bladeMatrix.Translate(128,128)
    $bladeMatrix.Rotate($angle)
    $bladeCopy.Transform($bladeMatrix)
    $iconGraphics.FillPath($iconLime,$bladeCopy)
    $bladeCopy.Dispose(); $bladeMatrix.Dispose()
}
$iconGraphics.FillEllipse($iconLime,112,112,32,32)
$iconHub = New-Object Drawing.SolidBrush ([Drawing.ColorTranslator]::FromHtml('#15191F'))
$iconGraphics.FillEllipse($iconHub,122,122,12,12)
$iconMaster.Save((Join-Path $iconOutputDirectory 'App.png'),[Drawing.Imaging.ImageFormat]::Png)
$iconSizes = @(16,24,32,48,64,128,256)
$iconFrames = @()
foreach ($iconSize in $iconSizes) {
    $iconBitmap = New-Object Drawing.Bitmap $iconSize,$iconSize
    $frameGraphics = [Drawing.Graphics]::FromImage($iconBitmap)
    $frameGraphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $frameGraphics.DrawImage($iconMaster,0,0,$iconSize,$iconSize)
    $frameStream = New-Object IO.MemoryStream
    $iconBitmap.Save($frameStream,[Drawing.Imaging.ImageFormat]::Png)
    $iconFrames += ,($frameStream.ToArray())
    $frameStream.Dispose(); $frameGraphics.Dispose(); $iconBitmap.Dispose()
}
$iconFile = [IO.File]::Create((Join-Path $iconOutputDirectory 'App.ico'))
$iconWriter = New-Object IO.BinaryWriter $iconFile
try {
    $iconWriter.Write([uint16]0); $iconWriter.Write([uint16]1); $iconWriter.Write([uint16]$iconSizes.Count)
    $iconOffset = 6 + 16 * $iconSizes.Count
    for ($frameIndex=0; $frameIndex -lt $iconSizes.Count; $frameIndex++) {
        $dimension = if ($iconSizes[$frameIndex] -eq 256) {0} else {$iconSizes[$frameIndex]}
        $iconWriter.Write([byte]$dimension); $iconWriter.Write([byte]$dimension)
        $iconWriter.Write([byte]0); $iconWriter.Write([byte]0)
        $iconWriter.Write([uint16]1); $iconWriter.Write([uint16]32)
        $iconWriter.Write([uint32]$iconFrames[$frameIndex].Length); $iconWriter.Write([uint32]$iconOffset)
        $iconOffset += $iconFrames[$frameIndex].Length
    }
    foreach ($iconFrame in $iconFrames) { $iconWriter.Write([byte[]]$iconFrame) }
} finally {
    $iconWriter.Dispose(); $iconFile.Dispose(); $iconGraphics.Dispose(); $iconMaster.Dispose()
    $iconBackground.Dispose(); $iconGradient.Dispose(); $iconBorder.Dispose(); $iconArc.Dispose()
    $iconBlade.Dispose(); $iconLime.Dispose(); $iconHub.Dispose()
}
Write-Output ('Generated icon sizes: ' + ($iconSizes -join ', '))
