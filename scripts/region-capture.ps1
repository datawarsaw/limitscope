param([string]$OutFile = "shot.png", [int]$X = 0, [int]$Y = 0, [int]$W = 700, [int]$H = 500)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;using System.Runtime.InteropServices;
public class RegionShot {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[RegionShot]::SetProcessDPIAware() | Out-Null
$bmp = New-Object System.Drawing.Bitmap($W, $H)
$g = [System.Drawing.Graphics]::CopyFromScreen($X, $Y, 0, 0, (New-Object System.Drawing.Size($W, $H)))
$g.Dispose()
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output ("SAVED " + $OutFile)
