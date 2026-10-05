# Dev helper: sends WM_CLOSE to the visible Rate Limits main window and reports
# whether it hid instead of exiting (close-to-tray verification).
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/close-window.ps1
param()

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Close {
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder sb, int max);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Find-MainWindow {
  Get-Process -Name "rate-limits" -ErrorAction SilentlyContinue | ForEach-Object {
    $hwnd = $_.MainWindowHandle
    if ($hwnd -eq [IntPtr]::Zero) { return }
    $title = New-Object System.Text.StringBuilder 256
    [Win32Close]::GetWindowTextW($hwnd, $title, 256) | Out-Null
    if ($title.ToString() -ne "Rate Limits") { return }
    if (-not [Win32Close]::IsWindowVisible($hwnd)) { return }
    $rect = New-Object Win32Close+RECT
    [Win32Close]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    if (($rect.Right - $rect.Left) -lt 200 -or ($rect.Bottom - $rect.Top) -lt 200) { return }
    return $hwnd
  } | Select-Object -First 1
}

$hwnd = Find-MainWindow
if (-not $hwnd) { Write-Output "window: not-found"; exit 1 }
$before = [Win32Close]::IsWindowVisible($hwnd)
[Win32Close]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 1000
$after = [Win32Close]::IsWindowVisible($hwnd)
Write-Output "visible-before: $before"
Write-Output "visible-after: $after"
