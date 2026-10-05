# Release-QA helper: right-clicks the Rate Limits tray icon and activates the
# "Quit" item of the Tauri popup menu via keyboard navigation ({UP} highlights
# the last item — Quit is last in Open | Refresh | — | Quit).
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tray-quit.ps1
param()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class TrayQuit {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  public static void LeftClick(int x, int y) {
    SetCursorPos(x, y); System.Threading.Thread.Sleep(120);
    mouse_event(2, 0, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(60);
    mouse_event(4, 0, 0, 0, UIntPtr.Zero);
  }
  public static void RightClick(int x, int y) {
    SetCursorPos(x, y); System.Threading.Thread.Sleep(120);
    mouse_event(8, 0, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(60);
    mouse_event(16, 0, 0, 0, UIntPtr.Zero);
  }
}
"@

$root = [System.Windows.Automation.AutomationElement]::RootElement

function Find-ByName([System.Windows.Automation.AutomationElement]$scope, [string]$name) {
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $name)
  return $scope.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

# 1. Open the overflow flyout and locate the tray icon.
$taskbar = $root.FindFirst([System.Windows.Automation.TreeScope]::Children,
  (New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'Shell_TrayWnd')))
$icon = $null
foreach ($attempt in 1..2) {
  if ($taskbar -ne $null) {
    $chevron = Find-ByName $taskbar 'Show Hidden Icons'
    if ($chevron -ne $null) {
      $r = $chevron.Current.BoundingRectangle
      [TrayQuit]::LeftClick([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
      Start-Sleep -Milliseconds 1200
    }
  }
  $tops = $root.FindAll([System.Windows.Automation.TreeScope]::Children,
    [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($top in $tops) {
    $icon = Find-ByName $top 'Rate Limits'
    if ($icon -ne $null) { break }
  }
  if ($icon -ne $null) { break }
}
if ($icon -eq $null) { Write-Output 'tray-icon: not-found'; exit 1 }
$ir = $icon.Current.BoundingRectangle
$ix = [int]($ir.X + $ir.Width / 2)
$iy = [int]($ir.Y + $ir.Height / 2)
Write-Output "tray-icon: found at ($ix,$iy)"

# 2. Right-click to open the Tauri popup menu, then Quit via keyboard:
#    one {UP} highlights the last item (Quit), {ENTER} activates it.
[TrayQuit]::RightClick($ix, $iy)
Start-Sleep -Milliseconds 900

# Diagnostic: dump any #32768 popup menu tree that is up right now.
$menus = $root.FindAll([System.Windows.Automation.TreeScope]::Children,
  (New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, '#32768')))
foreach ($m in $menus) {
  $off = $m.Current.IsOffscreen
  Write-Output "menu-window: present offscreen=$off"
  $kids = $m.FindAll([System.Windows.Automation.TreeScope]::Descendants,
    [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($k in $kids) {
    $c = $k.Current
    Write-Output ("  item type={0} name=`"{1}`" offscreen={2}" -f `
      $c.ControlType.ProgrammaticName, $c.Name, $c.IsOffscreen)
  }
}

[System.Windows.Forms.SendKeys]::SendWait('{UP}')
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
Write-Output 'sent: {UP}{ENTER}'
