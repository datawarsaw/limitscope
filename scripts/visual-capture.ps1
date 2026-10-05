# Visual acceptance capture: LimitScope main (>=200x200) / floating (<200 tall) windows.
param(
  [string]$OutFile = "shot.png",
  [switch]$Floating,
  [int]$ResizeW = 0,
  [int]$ResizeH = 0,
  [int]$MoveX = -100000,
  [int]$MoveY = -100000
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WinShot2 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
  public struct R { public int L, T, Rt, B; }
  public static IntPtr MainH = IntPtr.Zero;
  public static IntPtr FloatH = IntPtr.Zero;
  public static bool Cb(IntPtr h, IntPtr l) {
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (sb.ToString() == "LimitScope" && IsWindowVisible(h)) {
      R r; GetWindowRect(h, out r);
      int w = r.Rt - r.L, ht = r.B - r.T;
      if (w >= 200 && ht >= 200) MainH = h;
      else if (w >= 150 && ht <= 220) FloatH = h;
    }
    return true;
  }
}
"@
[WinShot2]::SetProcessDPIAware() | Out-Null
[WinShot2]::EnumWindows([WinShot2+EnumProc]{ param($h,$l) [WinShot2]::Cb($h,$l) }, [IntPtr]::Zero) | Out-Null
$hwnd = if ($Floating) { [WinShot2]::FloatH } else { [WinShot2]::MainH }
if ($hwnd -eq [IntPtr]::Zero) { Write-Output "WINDOW-NOT-FOUND"; exit 1 }
if ($ResizeW -gt 0 -or $MoveX -ne -100000) {
  $r = New-Object WinShot2+R
  [WinShot2]::GetWindowRect($hwnd, [ref]$r) | Out-Null
  $w = if ($ResizeW -gt 0) { $ResizeW } else { $r.Rt - $r.L }
  $hh = if ($ResizeH -gt 0) { $ResizeH } else { $r.B - $r.T }
  $x = if ($MoveX -ne -100000) { $MoveX } else { $r.L }
  $y = if ($MoveY -ne -100000) { $MoveY } else { $r.T }
  [WinShot2]::MoveWindow($hwnd, $x, $y, $w, $hh, $true) | Out-Null
  Start-Sleep -Milliseconds 900
}
$r = New-Object WinShot2+R
[WinShot2]::GetWindowRect($hwnd, [ref]$r) | Out-Null
$wd = $r.Rt - $r.L; $ht = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap($wd, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[WinShot2]::PrintWindow($hwnd, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc); $g.Dispose()
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output ("SAVED " + $OutFile + " " + $wd + "x" + $ht)
