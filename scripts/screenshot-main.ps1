# Dev helper: finds the "Rate Limits" main window (any process) and captures
# it to a PNG, bypassing the single-instance helper window.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/screenshot-main.ps1 -OutFile shot.png
param(
  [string]$OutFile = "shot.png"
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WinShot {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public struct R { public int L, T, Rt, B; }

  public static IntPtr Found = IntPtr.Zero;
  public static bool Cb(IntPtr h, IntPtr l) {
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (sb.ToString() == "Rate Limits" && IsWindowVisible(h)) {
      R r; GetWindowRect(h, out r);
      if (r.Rt - r.L >= 200 && r.B - r.T >= 200) { Found = h; return false; }
    }
    return true;
  }
  public static IntPtr Find() {
    Found = IntPtr.Zero;
    EnumProc d = (EnumProc)Delegate.CreateDelegate(typeof(EnumProc), typeof(WinShot).GetMethod("Cb"));
    EnumWindows(d, IntPtr.Zero);
    return Found;
  }
}
"@

[WinShot]::SetProcessDPIAware() | Out-Null
$hwnd = [WinShot]::Find()
if ($hwnd -eq [IntPtr]::Zero) { Write-Output "window: not-found"; exit 1 }
Start-Sleep -Milliseconds 600

$rect = New-Object WinShot+R
[WinShot]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$width = $rect.Rt - $rect.L
$height = $rect.B - $rect.T

$bmp = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $graphics.GetHdc()
$ok = [WinShot]::PrintWindow($hwnd, $hdc, 2) # PW_RENDERFULLCONTENT
$graphics.ReleaseHdc($hdc)
$graphics.Dispose()
if (-not $ok) { Write-Output "printwindow: failed"; exit 1 }
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved $OutFile ($width x $height)"
