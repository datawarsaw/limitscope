# Dev helper: captures the Rate Limits main window to a PNG (no focus steal).
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/screenshot-window.ps1 -OutFile shot.png
param(
  [string]$OutFile = "shot.png"
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Shot {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder sb, int max);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

[Win32Shot]::SetProcessDPIAware() | Out-Null

# Find a visible top-level window titled "Rate Limits" with a sane size
# (skips hidden helper windows like the tray icon's 16x16 message window).
function Find-MainWindow {
  Get-Process -Name "rate-limits" -ErrorAction SilentlyContinue | ForEach-Object {
    $hwnd = $_.MainWindowHandle
    if ($hwnd -eq [IntPtr]::Zero) { return }
    $title = New-Object System.Text.StringBuilder 256
    [Win32Shot]::GetWindowTextW($hwnd, $title, 256) | Out-Null
    if ($title.ToString() -ne "Rate Limits") { return }
    if (-not [Win32Shot]::IsWindowVisible($hwnd)) { return }
    $rect = New-Object Win32Shot+RECT
    [Win32Shot]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    if (($rect.Right - $rect.Left) -lt 200 -or ($rect.Bottom - $rect.Top) -lt 200) { return }
    return $hwnd
  } | Select-Object -First 1
}

$hwnd = Find-MainWindow
if (-not $hwnd) { Write-Output "window: not-found"; exit 1 }
Start-Sleep -Milliseconds 600

$rect = New-Object Win32Shot+RECT
[Win32Shot]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

# PrintWindow renders the window's own surface, so the capture works even when
# the window is behind other windows and nothing is stolen from the user.
$bmp = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $graphics.GetHdc()
$ok = [Win32Shot]::PrintWindow($hwnd, $hdc, 2) # PW_RENDERFULLCONTENT
$graphics.ReleaseHdc($hdc)
$graphics.Dispose()
if (-not $ok) { Write-Output "printwindow: failed"; exit 1 }
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved $OutFile ($width x $height)"
