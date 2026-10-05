# Release-QA helper: opens the Win11 "hidden icons" overflow flyout and
# searches every top-level window for the Rate Limits tray icon.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tray-uia-overflow.ps1
param()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class TrayClick {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  public static void Click(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(120);
    mouse_event(2, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    mouse_event(4, 0, 0, 0, UIntPtr.Zero);
  }
  public static void RightClick(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(120);
    mouse_event(8, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    mouse_event(16, 0, 0, 0, UIntPtr.Zero);
  }
}
"@

$root = [System.Windows.Automation.AutomationElement]::RootElement

function Find-ByNameInScope([System.Windows.Automation.AutomationElement]$scope, [string]$name) {
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $name)
  return $scope.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

# 1. Open the overflow flyout via the taskbar chevron ("Show Hidden Icons").
$taskbar = $root.FindFirst([System.Windows.Automation.TreeScope]::Children,
  (New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'Shell_TrayWnd')))
if ($taskbar -ne $null) {
  $chevron = Find-ByNameInScope $taskbar 'Show Hidden Icons'
  if ($chevron -ne $null) {
    $r = $chevron.Current.BoundingRectangle
    [TrayClick]::Click([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
    Start-Sleep -Milliseconds 1200
    Write-Output 'chevron: clicked'
  } else {
    Write-Output 'chevron: not-found'
  }
} else {
  Write-Output 'taskbar: not-found'
}

# 2. Search all top-level windows for the icon element named "Rate Limits".
$tops = $root.FindAll([System.Windows.Automation.TreeScope]::Children,
  [System.Windows.Automation.Condition]::TrueCondition)
foreach ($top in $tops) {
  $el = Find-ByNameInScope $top 'Rate Limits'
  if ($el -ne $null) {
    $c = $el.Current
    $rect = $c.BoundingRectangle
    Write-Output ('overflow-icon: found type={0} class={1} in-window-class={2} center=({3},{4}) size={5}x{6}' -f `
      $c.ControlType.ProgrammaticName, $c.ClassName, $top.Current.ClassName,
      [int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2),
      [int]$rect.Width, [int]$rect.Height)
    exit 0
  }
}
Write-Output 'overflow-icon: not-found'
exit 1
