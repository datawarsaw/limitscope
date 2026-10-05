# Validation helper: clicks a point in the "Rate Limits" window's CLIENT area.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/click-client.ps1 -X 399 -Y 13
param(
  [int]$X = 0,
  [int]$Y = 0
)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WinClick {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  public struct R { public int W, H; }
  public struct P { public int X, Y; }
  public static IntPtr Found = IntPtr.Zero;
  public static bool Cb(IntPtr h, IntPtr l) {
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (sb.ToString() == "Rate Limits" && IsWindowVisible(h)) { Found = h; return false; }
    return true;
  }
  public static IntPtr Find() {
    Found = IntPtr.Zero;
    EnumProc d = (EnumProc)Delegate.CreateDelegate(typeof(EnumProc), typeof(WinClick).GetMethod("Cb"));
    EnumWindows(d, IntPtr.Zero);
    return Found;
  }
}
"@
$hwnd = [WinClick]::Find()
if ($hwnd -eq [IntPtr]::Zero) { Write-Output "window: not-found"; exit 1 }
$rect = New-Object WinClick+R
[WinClick]::GetClientRect($hwnd, [ref]$rect) | Out-Null
$pt = New-Object WinClick+P
$pt.X = $X; $pt.Y = $Y
[WinClick]::ClientToScreen($hwnd, [ref]$pt) | Out-Null
[WinClick]::SetCursorPos($pt.X, $pt.Y) | Out-Null
Start-Sleep -Milliseconds 120
[WinClick]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)  # left down
Start-Sleep -Milliseconds 60
[WinClick]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)  # left up
Write-Output "clicked client($X,$Y) screen($($pt.X),$($pt.Y)) client=$($rect.W)x$($rect.H)"
