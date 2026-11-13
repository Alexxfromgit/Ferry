# Generates Ferry's icons from the source art (Windows, System.Drawing; no extra installs).
#
# App icon, from app/src-tauri/icons/source.png:
#   - a rounded-corner master (corners 20% of the side, transparent outside), which
#     `tauri icon` turns into every platform size (icns, PNGs, Windows Store logos);
#   - app/src-tauri/icons/icon.ico, written here instead: sizes up to 24 px use a tighter crop
#     around the face, so the tray and small title-bar icons stay recognizable;
#   - app/public/ferry-icon.png (256 px), the logo shown inside the app.
#
# .ferry file icon, from app/src-tauri/icons/ferry-file-source.png:
#   the art is a finished document page (ticket, FERRY label, curled corner) on a dark
#   background; the page is cut out along its measured outline and placed on a transparent
#   square with a soft shadow. Writes app/src-tauri/icons/ferry-file.ico and ferry-file.png.
#
# Usage (from the repo root): pwsh scripts/make-icons.ps1
#   [-CornerRadius 0.2] [-AppSmallCrop 0.27,0.07,0.46]
param(
    [double] $CornerRadius = 0.2,
    # x, y and side of the small app-icon crop, as fractions of the source width
    [double[]] $AppSmallCrop = @(0.27, 0.07, 0.46)
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$icons = Join-Path $root 'app\src-tauri\icons'

# ------------------------------------------------------------------ helpers

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function New-Graphics([System.Drawing.Bitmap]$bmp) {
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $g.CompositingQuality = 'HighQuality'
    $g.TextRenderingHint = 'AntiAliasGridFit'
    return $g
}

function New-Canvas([int]$size) {
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = New-Graphics $bmp
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.Dispose()
    return $bmp
}

# Draw part of $art into $dest, clipped to $path (anti-aliased edges via a texture brush).
function Fill-PathWithArt($g, $path, $art, [System.Drawing.RectangleF]$dest, [System.Drawing.Rectangle]$src) {
    $tile = [System.Drawing.Bitmap]::new([int][Math]::Ceiling($dest.Width), [int][Math]::Ceiling($dest.Height))
    $tg = New-Graphics $tile
    $attrs = [System.Drawing.Imaging.ImageAttributes]::new()
    $attrs.SetWrapMode('TileFlipXY') # no dark fringes at the edges
    $tg.DrawImage($art, [System.Drawing.Rectangle]::new(0, 0, $tile.Width, $tile.Height),
        $src.X, $src.Y, $src.Width, $src.Height, 'Pixel', $attrs)
    $tg.Dispose()
    $brush = [System.Drawing.TextureBrush]::new($tile)
    $brush.TranslateTransform($dest.X, $dest.Y)
    $g.FillPath($brush, $path)
    $brush.Dispose()
    $tile.Dispose()
}

function Resize-Bitmap([System.Drawing.Bitmap]$bmp, [int]$size) {
    $out = New-Canvas $size
    $g = New-Graphics $out
    $attrs = [System.Drawing.Imaging.ImageAttributes]::new()
    $attrs.SetWrapMode('TileFlipXY')
    $g.DrawImage($bmp, [System.Drawing.Rectangle]::new(0, 0, $size, $size), 0, 0, $bmp.Width, $bmp.Height, 'Pixel', $attrs)
    $g.Dispose()
    return $out
}

function Save-Ico([System.Drawing.Bitmap[]]$images, [string]$path) {
    # ICO with PNG-compressed entries (supported since Windows Vista).
    $pngs = foreach ($img in $images) {
        $ms = [System.IO.MemoryStream]::new()
        $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        , $ms.ToArray()
    }
    $out = [System.IO.MemoryStream]::new()
    $w = [System.IO.BinaryWriter]::new($out)
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$images.Count)
    $offset = 6 + 16 * $images.Count
    for ($i = 0; $i -lt $images.Count; $i++) {
        $s = $images[$i].Width; $len = $pngs[$i].Length
        $dim = if ($s -ge 256) { 0 } else { $s }
        $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$len); $w.Write([uint32]$offset)
        $offset += $len
    }
    foreach ($png in $pngs) { $w.Write($png) }
    $w.Flush()
    [System.IO.File]::WriteAllBytes($path, $out.ToArray())
}

$IcoSizes = 16, 20, 24, 32, 40, 48, 64, 128, 256

# ---------------------------------------------------------------- app icon

function New-AppIcon($art, [int]$size, [bool]$small) {
    $src = if ($small) {
        [System.Drawing.Rectangle]::new([int]($art.Width * $AppSmallCrop[0]), [int]($art.Width * $AppSmallCrop[1]),
            [int]($art.Width * $AppSmallCrop[2]), [int]($art.Width * $AppSmallCrop[2]))
    } else {
        [System.Drawing.Rectangle]::new(0, 0, $art.Width, $art.Height)
    }
    $bmp = New-Canvas $size
    $g = New-Graphics $bmp
    $path = New-RoundedRect 0 0 $size $size ($size * $CornerRadius)
    Fill-PathWithArt $g $path $art ([System.Drawing.RectangleF]::new(0, 0, $size, $size)) $src
    $g.Dispose()
    return $bmp
}

$appArt = [System.Drawing.Image]::FromFile((Join-Path $icons 'source.png'))
$master = New-AppIcon $appArt 1024 $false
$masterPath = Join-Path $icons 'source-rounded.png'
$master.Save($masterPath, [System.Drawing.Imaging.ImageFormat]::Png)

# All platform sizes from the rounded master; icon.ico is replaced below.
Push-Location (Join-Path $root 'app')
try {
    & npx tauri icon $masterPath -o $icons | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "tauri icon failed" }
} finally {
    Pop-Location
    Remove-Item $masterPath -ErrorAction SilentlyContinue # intermediate only
}

$appImages = foreach ($s in $IcoSizes) {
    if ($s -le 24) { New-AppIcon $appArt $s $true } else { Resize-Bitmap $master $s }
}
Save-Ico $appImages (Join-Path $icons 'icon.ico')
$publicLogo = Resize-Bitmap $master 256
$publicLogo.Save((Join-Path $root 'app\public\ferry-icon.png'), [System.Drawing.Imaging.ImageFormat]::Png)
$appArt.Dispose()
Write-Host "App icon: icon.ico, platform sizes and app/public/ferry-icon.png"

# ----------------------------------------------------------- .ferry file icon

# The source art is a finished document page (rounded corners, curled top-right corner) on a
# dark background. It is cut out along its outline, measured in source pixels, and placed on a
# transparent square with a soft shadow. If the art changes, measure these again.
$Page = @{ Left = 97; Top = 81; Right = 1034.5; Bottom = 1313; Radius = 147 }
# The curled corner's outer edge, from the top edge down to the right edge (x, y).
$Curl = @(@(740, 81), @(746, 84), @(776, 100), @(798, 120), @(828, 150), @(876, 200), @(934, 260),
    @(973, 300), @(1000, 330), @(1015, 350), @(1026, 370), @(1032, 390), @(1034.5, 404))

function New-PageOutline {
    $l = $Page.Left; $t = $Page.Top; $r = $Page.Right; $b = $Page.Bottom; $d = $Page.Radius * 2
    $p = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $p.AddArc($l, $t, $d, $d, 180, 90)
    $pts = [System.Drawing.PointF[]]($Curl | ForEach-Object { [System.Drawing.PointF]::new($_[0], $_[1]) })
    $p.AddLine($l + $Page.Radius, $t, $pts[0].X, $pts[0].Y)
    $p.AddCurve($pts, 0.3)
    $p.AddLine($pts[-1].X, $pts[-1].Y, $r, $b - $Page.Radius)
    $p.AddArc($r - $d, $b - $d, $d, $d, 0, 90)
    $p.AddArc($l, $b - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function New-FileIcon($art) {
    $outline = New-PageOutline
    # Cut the page out at full source resolution, with anti-aliased edges.
    $w = [int][Math]::Ceiling($Page.Right - $Page.Left); $h = [int][Math]::Ceiling($Page.Bottom - $Page.Top)
    $cut = New-Canvas ([Math]::Max($w, $h))
    $cg = New-Graphics $cut
    $brush = [System.Drawing.TextureBrush]::new($art)
    $brush.TranslateTransform(-$Page.Left, -$Page.Top)
    $m = [System.Drawing.Drawing2D.Matrix]::new()
    $m.Translate(-$Page.Left, -$Page.Top)
    $local = $outline.Clone(); $local.Transform($m)
    $cg.FillPath($brush, $local)
    $cg.Dispose()

    # Place it on a 1024 canvas: page height 944, centred, shadow below.
    $n = 1024
    $scale = 944 / $h
    $dw = $w * $scale; $dh = $h * $scale
    $dx = ($n - $dw) / 2; $dy = ($n - $dh) / 2 - 8
    $bmp = New-Canvas $n
    $g = New-Graphics $bmp
    $place = [System.Drawing.Drawing2D.Matrix]::new()
    $place.Translate($dx, $dy); $place.Scale($scale, $scale)
    for ($i = 10; $i -ge 1; $i--) {
        $shadow = $local.Clone()
        $sm = [System.Drawing.Drawing2D.Matrix]::new()
        $grow = 1 + $i * 0.004
        $sm.Translate($dx + $dw * (1 - $grow) / 2, $dy + 12 + $dh * (1 - $grow) / 2)
        $sm.Scale($scale * $grow, $scale * $grow)
        $shadow.Transform($sm)
        $g.FillPath([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(8, 30, 0, 30)), $shadow)
    }
    $g.DrawImage($cut, [System.Drawing.RectangleF]::new($dx, $dy, $cut.Width * $scale, $cut.Height * $scale))
    $g.Dispose()
    $cut.Dispose()
    return $bmp
}

$ticketArt = [System.Drawing.Bitmap]::new((Join-Path $icons 'ferry-file-source.png'))
$fileIcon = New-FileIcon $ticketArt
$fileImages = foreach ($s in $IcoSizes) { Resize-Bitmap $fileIcon $s }
Save-Ico $fileImages (Join-Path $icons 'ferry-file.ico')
($fileImages | Where-Object { $_.Width -eq 256 }).Save((Join-Path $icons 'ferry-file.png'), [System.Drawing.Imaging.ImageFormat]::Png)
$ticketArt.Dispose()
Write-Host "File icon: ferry-file.ico and ferry-file.png"
