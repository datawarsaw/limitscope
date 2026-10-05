param([string]$Action, [string]$Target)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;using System.Text;using System.Runtime.InteropServices;
public class Drive2 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static void Click(int x, int y) { SetCursorPos(x,y); System.Threading.Thread.Sleep(140); mouse_event(2,0,0,0,UIntPtr.Zero); System.Threading.Thread.Sleep(70); mouse_event(4,0,0,0,UIntPtr.Zero); }
  public static void Key(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(90); keybd_event(vk, 0, 2, UIntPtr.Zero); }
}
"@
[Drive2]::SetProcessDPIAware() | Out-Null
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'LimitScope')
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
$main = $null
foreach ($w in $wins) {
  if ($w.Current.ClassName -eq 'Tauri Window' -and $w.Current.BoundingRectangle.Height -ge 200) { $main = $w; break }
}
if (-not $main) { Write-Output 'NO-WINDOW'; exit 1 }

function Find-ByName([string]$name) {
  $main.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $name)))
}
function Click-El($el) {
  $r = $el.Current.BoundingRectangle
  [Drive2]::Click([int]($r.X + $r.Width/2), [int]($r.Y + $r.Height/2))
}

switch ($Action) {
  'dump' {
    $els = $main.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($e in $els) { $c = $e.Current; if ($c.Name) { Write-Output ("[" + $c.ControlType.ProgrammaticName + "] '" + $c.Name + "'") } }
  }
  'click' {
    $el = Find-ByName $Target
    if ($el) { Click-El $el; Write-Output ("clicked '" + $Target + "'") } else { Write-Output ("NOT-FOUND '" + $Target + "'"); exit 1 }
  }
  'themeselect' {
    # open the theme combobox (a <select> renders as ComboBox; find by near the Theme text is fragile -
    # instead find the ComboBox control and click it)
    $cb = $main.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.AndCondition(
      (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::ComboBox)),
      (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'Theme')))))
    if (-not $cb) { Write-Output 'NO-COMBO'; exit 1 }
    Click-El $cb
    Start-Sleep -Milliseconds 800
    # native list popup: choose by first letter then Enter
    $letter = switch ($Target) { 'graphite' {0x47} 'glass' {0x47} 'oled' {0x4F} }
    [Drive2]::Key($letter)
    if ($Target -eq 'glass') { Start-Sleep -Milliseconds 300; [Drive2]::Key($letter) }
    Start-Sleep -Milliseconds 400
    [Drive2]::Key(0x0D)
    Start-Sleep -Milliseconds 600
    Write-Output ("theme->" + $Target)
  }
  'drawer' {
    # $Target = 'open' or 'close', verified with retries
    for ($i = 0; $i -lt 4; $i++) {
      $radio = $main.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'Used')))
      $isOpen = ($radio -ne $null)
      if (($Target -eq 'open' -and $isOpen) -or ($Target -eq 'close' -and -not $isOpen)) { Write-Output ("drawer=" + $Target + " ok"); break }
      $btn = Find-ByName 'Settings'
      if ($btn) { Click-El $btn }
      Start-Sleep -Milliseconds 1100
    }
  }
  'clickmain' {
    Add-Type @"
using System;using System.Text;using System.Runtime.InteropServices;
public class WRect {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
  public struct R { public int L, T, Rt, B; }
  public static IntPtr MainH = IntPtr.Zero;
  public static bool Cb(IntPtr h, IntPtr l) {
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (sb.ToString() == "LimitScope" && IsWindowVisible(h)) {
      R r; GetWindowRect(h, out r);
      if (r.Rt - r.L >= 200 && r.B - r.T >= 200) { MainH = h; return false; }
    }
    return true;
  }
}
"@
    [WRect]::EnumWindows([WRect+EnumProc]{ param($h,$l) [WRect]::Cb($h,$l) }, [IntPtr]::Zero) | Out-Null
    $r = New-Object WRect+R
    [WRect]::GetWindowRect([WRect]::MainH, [ref]$r) | Out-Null
    $parts = $Target.Split(',')
    [Drive2]::Click($r.L + [int]$parts[0], $r.T + [int]$parts[1])
    Write-Output ("clickmain " + $Target + " at " + ($r.L + [int]$parts[0]) + "," + ($r.T + [int]$parts[1]))
  }
  'esc' {
    [Drive2]::Key(0x1B)
    Start-Sleep -Milliseconds 500
    Write-Output 'esc sent'
  }
  default { Write-Output 'BAD-ACTION'; exit 1 }
}
