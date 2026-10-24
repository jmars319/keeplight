# Draws the app icon (a lit LED with a halo, above a strip of three LEDs) and
# writes assets/icon-256.png plus a multi-size assets/app.ico. Windows only.
# Same Vaexil canvas as Modlist Launcher's icon, with its own symbol, so the
# apps stay distinct in the taskbar.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$assets = Join-Path $PSScriptRoot '..\assets'
New-Item -ItemType Directory -Force $assets | Out-Null

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

function New-Circle([float]$cx, [float]$cy, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddEllipse($cx - $r, $cy - $r, $r * 2, $r * 2)
    $p
}

function Draw-Icon([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = $size / 256.0

    # Vaexil: the #05070d canvas with the mark's blue-to-magenta.
    $blue = [System.Drawing.Color]::FromArgb(255, 47, 123, 255)
    $magenta = [System.Drawing.Color]::FromArgb(255, 255, 63, 216)
    $cyan = { param($a) [System.Drawing.Color]::FromArgb($a, 103, 232, 249) }
    $diag = { param($a, $b, $x0, $y0, $x1, $y1) New-Object System.Drawing.Drawing2D.LinearGradientBrush (
        (New-Object System.Drawing.PointF $x0, $y0), (New-Object System.Drawing.PointF $x1, $y1), $a, $b) }

    $bg = New-RoundedRect (8 * $s) (8 * $s) (240 * $s) (240 * $s) (54 * $s)
    $g.FillPath((& $diag ([System.Drawing.Color]::FromArgb(255, 11, 22, 40)) ([System.Drawing.Color]::FromArgb(255, 5, 7, 13)) 0 0 $size $size), $bg)
    $g.DrawPath((New-Object System.Drawing.Pen ((& $diag $blue $magenta 0 0 $size $size), [Math]::Max(1, 5 * $s))), $bg)

    # The lit LED: two soft halo rings, then the blue-to-magenta core.
    $cx = 128 * $s; $cy = 104 * $s
    $g.FillPath((New-Object System.Drawing.SolidBrush (& $cyan 40)), (New-Circle $cx $cy (66 * $s)))
    $g.FillPath((New-Object System.Drawing.SolidBrush (& $cyan 80)), (New-Circle $cx $cy (48 * $s)))
    $g.FillPath((& $diag $blue $magenta ($cx - 32 * $s) ($cy - 32 * $s) ($cx + 32 * $s) ($cy + 32 * $s)), (New-Circle $cx $cy (32 * $s)))
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(230, 248, 250, 252))), (New-Circle ($cx - 10 * $s) ($cy - 10 * $s) (9 * $s)))

    # The strip it sits above: three LEDs, dimmer toward the edges.
    $strip = New-RoundedRect (52 * $s) (186 * $s) (152 * $s) (30 * $s) (15 * $s)
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 21, 30, 46))), $strip)
    foreach ($led in @(@{ X = 84; A = 150 }, @{ X = 128; A = 255 }, @{ X = 172; A = 150 })) {
        $g.FillPath((New-Object System.Drawing.SolidBrush (& $cyan $led.A)), (New-Circle ($led.X * $s) (201 * $s) (8 * $s)))
    }
    $g.Dispose()
    $bmp
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @{}
foreach ($size in $sizes) {
    $bmp = Draw-Icon $size
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs[$size] = $ms.ToArray()
    if ($size -eq 256) { $bmp.Save((Join-Path $assets 'icon-256.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
    $bmp.Dispose()
}

# ICO container with PNG-compressed entries.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($size in $sizes) {
    $dim = if ($size -ge 256) { 0 } else { $size }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$size].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$size].Length
}
foreach ($size in $sizes) { $w.Write($pngs[$size]) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $assets 'app.ico'), $out.ToArray())
Write-Output "Wrote $assets\icon-256.png and app.ico"
