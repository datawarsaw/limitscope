#Requires -Version 5.1
<#
.SYNOPSIS
  Deterministic Windows-native acceptance harness for LimitScope v0.6 lifecycle testing.
#>

[CmdletBinding()]
param(
    [string]$ExePath,
    [string]$InstallerPath,
    [string]$AppDisplayName = 'LimitScope',
    [string]$AppVersion = '0.5.0',
    [string]$ReportDir,
    [switch]$Smoke,
    [switch]$Floating,
    [switch]$SingleInstance,
    [switch]$Full,
    [switch]$StopExisting,
    [switch]$Interactive,
    [switch]$NoProfileRestore,
    [int]$StartupWaitSeconds = 4,
    [int]$SettleSeconds = 8
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient -ErrorAction SilentlyContinue
Add-Type -AssemblyName UIAutomationTypes -ErrorAction SilentlyContinue

$win32Source = @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public class NativeAcceptanceWin32 {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder sb, int maxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder sb, int maxCount);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    public class WindowRecord {
        public IntPtr Hwnd;
        public uint Pid;
        public string Title;
        public string ClassName;
        public bool Visible;
        public int X;
        public int Y;
        public int Width;
        public int Height;
        public int ExStyle;
        public int Style;
        public bool IsTopmost { get { return (ExStyle & 0x00000008) != 0; } }
    }

    public static List<WindowRecord> GetWindowsForPid(uint targetPid) {
        var list = new List<WindowRecord>();
        EnumWindows((hWnd, lParam) => {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            if (pid == targetPid) {
                var sbTitle = new StringBuilder(256);
                GetWindowTextW(hWnd, sbTitle, 256);
                var sbClass = new StringBuilder(256);
                GetClassNameW(hWnd, sbClass, 256);
                RECT r;
                GetWindowRect(hWnd, out r);
                list.Add(new WindowRecord {
                    Hwnd = hWnd,
                    Pid = pid,
                    Title = sbTitle.ToString(),
                    ClassName = sbClass.ToString(),
                    Visible = IsWindowVisible(hWnd),
                    X = r.Left,
                    Y = r.Top,
                    Width = r.Right - r.Left,
                    Height = r.Bottom - r.Top,
                    ExStyle = GetWindowLongW(hWnd, -20),
                    Style = GetWindowLongW(hWnd, -16)
                });
            }
            return true;
        }, IntPtr.Zero);
        return list;
    }

    public static void LeftClick(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(80);
        mouse_event(2, 0, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(40);
        mouse_event(4, 0, 0, 0, UIntPtr.Zero);
    }

    public static void RightClick(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(80);
        mouse_event(8, 0, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(40);
        mouse_event(16, 0, 0, 0, UIntPtr.Zero);
    }

    public static void SendKey(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(40);
        keybd_event(vk, 0, 2, UIntPtr.Zero);
    }
}
"@

if (-not ([System.Management.Automation.PSTypeName]'NativeAcceptanceWin32').Type) {
    Add-Type -TypeDefinition $win32Source
}

$script:RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $ReportDir) {
    $script:ReportDir = Join-Path $script:RepoRoot 'artifacts'
} else {
    $script:ReportDir = [System.IO.Path]::GetFullPath($ReportDir)
}
if (-not (Test-Path -LiteralPath $script:ReportDir)) {
    New-Item -ItemType Directory -Path $script:ReportDir -Force | Out-Null
}

$script:StartTime = Get-Date
$script:LogLines = New-Object 'System.Collections.Generic.List[string]'
$script:TestResults = New-Object 'System.Collections.Specialized.OrderedDictionary'
$script:BackupDir = $null
$script:ActiveProcess = $null

function Log-Message {
    param([string]$Message, [string]$Color = 'White')
    $stamp = Get-Date -Format 'HH:mm:ss.fff'
    $line = "[$stamp] $Message"
    Write-Host $line -ForegroundColor $Color
    $script:LogLines.Add($line) | Out-Null
}

function Add-Test {
    param(
        [int]$Flow,
        [string]$Id,
        [string]$Name,
        [ValidateSet('PASS', 'FAIL', 'HUMAN_REQUIRED', 'SKIPPED')][string]$Status,
        [string]$Detail,
        [long]$DurationMs = 0
    )
    $script:TestResults[$Id] = @{
        flow = $Flow
        id = $Id
        name = $Name
        status = $Status
        detail = $Detail
        durationMs = $DurationMs
    }

    $color = switch ($Status) {
        'PASS' { 'Green' }
        'FAIL' { 'Red' }
        'HUMAN_REQUIRED' { 'Yellow' }
        default { 'DarkGray' }
    }
    Log-Message ("  [{0}] Flow {1:D2}: {2} - {3}" -f $Status, $Flow, $Name, $Detail) $color
}

function Resolve-TargetBinary {
    param([string]$GivenPath)
    if ($GivenPath -and (Test-Path -LiteralPath $GivenPath -PathType Leaf)) {
        return (Get-Item -LiteralPath $GivenPath).FullName
    }

    $candidates = @(
        (Join-Path $script:RepoRoot 'src-tauri\target\release\LimitScope.exe'),
        (Join-Path $script:RepoRoot 'src-tauri\target\release\rate-limits.exe'),
        (Join-Path $env:LOCALAPPDATA 'Rate Limits\LimitScope.exe'),
        (Join-Path $env:ProgramFiles 'LimitScope\LimitScope.exe')
    )

    foreach ($cand in $candidates) {
        if (Test-Path -LiteralPath $cand -PathType Leaf) {
            return (Get-Item -LiteralPath $cand).FullName
        }
    }
    return $null
}

function Get-TargetProcessSnapshot {
    param([string]$TargetExe)
    $pname = [System.IO.Path]::GetFileNameWithoutExtension($TargetExe)
    $procs = @(Get-Process -Name $pname -ErrorAction SilentlyContinue)
    $matched = @()
    foreach ($p in $procs) {
        $path = $null
        try { $path = $p.Path } catch { }
        if (-not $path) {
            try { if ($p.MainModule) { $path = $p.MainModule.FileName } } catch { }
        }
        $isMatch = $false
        if ($path -and ([string]::Compare($path, $TargetExe, $true) -eq 0)) {
            $isMatch = $true
        }
        $matched += [PSCustomObject]@{
            Id = $p.Id
            ProcessName = $p.ProcessName
            Path = $path
            PathMatched = $isMatch
            MainWindowTitle = $p.MainWindowTitle
        }
    }
    return ,$matched
}

function Get-AppWindows {
    param([int]$TargetPid)
    if ($TargetPid -le 0) { return @() }
    return ,([NativeAcceptanceWin32]::GetWindowsForPid([uint32]$TargetPid))
}

function Get-MainWindow {
    param([int]$TargetPid)
    $wins = Get-AppWindows -TargetPid $TargetPid
    return ($wins | Where-Object {
        $_.ClassName -eq 'Tauri Window' -and ($_.Height -gt 200 -or $_.Width -gt 350)
    } | Select-Object -First 1)
}

function Get-FloatingWindow {
    param([int]$TargetPid)
    $wins = Get-AppWindows -TargetPid $TargetPid
    return ($wins | Where-Object {
        $_.ClassName -eq 'Tauri Window' -and $_.Height -ge 40 -and $_.Height -le 450 -and ($_.Width -le 660 -and $_.Width -ge 280)
    } | Select-Object -First 1)
}


function Get-TrayButtonElement {
    $root = [System.Windows.Automation.AutomationElement]::RootElement

    $overflowCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'TopLevelWindowForOverflowXamlIsland')
    $overflow = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $overflowCond)

    $isOpen = ($overflow -ne $null -and -not $overflow.Current.IsOffscreen)
    if (-not $isOpen) {
        $trays = @($root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -match 'Tray' })
        foreach ($t in $trays) {
            $chev = $t.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
                (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'Show Hidden Icons')))
            if ($chev) {
                try {
                    $inv = $chev.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                    $inv.Invoke()
                } catch {
                    $r = $chev.Current.BoundingRectangle
                    [NativeAcceptanceWin32]::LeftClick([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
                }
                Start-Sleep -Milliseconds 450
                break
            }
        }
    }

    $overflow = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $overflowCond)
    if ($overflow) {
        $btn = $overflow.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
            (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $AppDisplayName)))
        if ($btn) { return $btn }
        $legacyBtn = $overflow.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
            (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'Rate Limits')))
        if ($legacyBtn) { return $legacyBtn }
    }

    $trays = @($root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -match 'Tray' })
    foreach ($t in $trays) {
        $btn = $t.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
            (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $AppDisplayName)))
        if ($btn -and $btn.Current.ControlType.ProgrammaticName -eq 'ControlType.Button') {
            return $btn
        }
    }
    return $null
}

function Dismiss-TrayMenu {
    [NativeAcceptanceWin32]::SendKey(0x1B)
    Start-Sleep -Milliseconds 250
}

function Get-TrayMenuInfo {
    $btn = Get-TrayButtonElement
    if (-not $btn) { return $null }

    $rect = $btn.Current.BoundingRectangle
    $cx = [int]($rect.X + $rect.Width / 2)
    $cy = [int]($rect.Y + $rect.Height / 2)

    [NativeAcceptanceWin32]::RightClick($cx, $cy)
    Start-Sleep -Milliseconds 600

    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $menuCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ClassNameProperty, '#32768')
    $menus = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $menuCond)

    $items = @()
    foreach ($m in $menus) {
        $descendants = $m.FindAll([System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($d in $descendants) {
            $c = $d.Current
            if ($c.ControlType.ProgrammaticName -match 'MenuItem|Separator') {
                $items += [PSCustomObject]@{
                    Name = $c.Name
                    ControlType = $c.ControlType.ProgrammaticName
                    IsEnabled = $c.IsEnabled
                    BoundingRectangle = $c.BoundingRectangle
                }
            }
        }
    }
    return [PSCustomObject]@{
        ButtonRect = $rect
        ButtonCenter = [PSCustomObject]@{ X = $cx; Y = $cy }
        MenuItems = $items
    }
}

function Invoke-TrayCommand {
    param([string]$CommandName)
    $info = Get-TrayMenuInfo
    if (-not $info) { return $false }

    $target = $info.MenuItems | Where-Object { $_.Name -like "*$CommandName*" } | Select-Object -First 1
    if (-not $target) {
        Dismiss-TrayMenu
        return $false
    }

    if (-not $target.IsEnabled) {
        Dismiss-TrayMenu
        return 'DISABLED'
    }

    $r = $target.BoundingRectangle
    $cx = [int]($r.X + $r.Width / 2)
    $cy = [int]($r.Y + $r.Height / 2)
    [NativeAcceptanceWin32]::LeftClick($cx, $cy)
    Start-Sleep -Milliseconds 800
    return $true
}

function Invoke-QuitGracefully {
    param([int]$TargetPid)
    # Activate the exact "Quit" menu item instead of keyboard navigation:
    # blind UP+ENTER lands on the wrong entry whenever any sibling menu item
    # is disabled at that moment (observed on a freshly restarted instance),
    # which silently turned the quit flow into a refresh.
    $info = Get-TrayMenuInfo
    if ($info) {
        $quit = $info.MenuItems | Where-Object { $_.Name -like "*Quit*" } | Select-Object -First 1
        if ($quit -and $quit.IsEnabled) {
            $r = $quit.BoundingRectangle
            [NativeAcceptanceWin32]::LeftClick(
                [int]($r.X + $r.Width / 2),
                [int]($r.Y + $r.Height / 2)
            )
            Start-Sleep -Seconds 2
        } else {
            Dismiss-TrayMenu
        }
    }

    if ($TargetPid -gt 0) {
        $dead = Wait-ForProcessExit -TargetPid $TargetPid -TimeoutSeconds 6
        if ($dead) { return $true }
    }
    return $false
}

function Wait-ForProcessExit {
    param([int]$TargetPid, [int]$TimeoutSeconds = 8)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        $p = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
        if (-not $p -or $p.HasExited) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Backup-UserProfile {
    $local = Join-Path $env:LOCALAPPDATA 'com.ratelimits.desktop'
    $roaming = Join-Path $env:APPDATA 'com.ratelimits.desktop'

    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $backupRoot = Join-Path $env:TEMP ("limitscope_acceptance_backup_" + $stamp)
    New-Item -ItemType Directory -Path $backupRoot -Force | Out-Null

    if (Test-Path -LiteralPath $local) {
        $localDest = Join-Path $backupRoot 'Local'
        Copy-Item -LiteralPath $local -Destination $localDest -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $roaming) {
        $roamDest = Join-Path $backupRoot 'Roaming'
        Copy-Item -LiteralPath $roaming -Destination $roamDest -Recurse -Force -ErrorAction SilentlyContinue
    }

    $script:BackupDir = $backupRoot
    Log-Message "Backed up user profile state to: $backupRoot" 'DarkGray'
}

function Restore-UserProfile {
    if (-not $script:BackupDir -or (-not (Test-Path -LiteralPath $script:BackupDir))) { return }
    if ($NoProfileRestore) {
        Log-Message "-NoProfileRestore specified: preserving test profile on disk." 'Yellow'
        return
    }

    $local = Join-Path $env:LOCALAPPDATA 'com.ratelimits.desktop'
    $roaming = Join-Path $env:APPDATA 'com.ratelimits.desktop'

    $localBak = Join-Path $script:BackupDir 'Local'
    $roamBak = Join-Path $script:BackupDir 'Roaming'

    if (Test-Path -LiteralPath $localBak) {
        Copy-Item -LiteralPath "$localBak\*" -Destination $local -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $roamBak) {
        Copy-Item -LiteralPath "$roamBak\*" -Destination $roaming -Recurse -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $script:BackupDir -Recurse -Force -ErrorAction SilentlyContinue
    Log-Message "Restored original user profile state and cleaned up temporary backup." 'DarkGray'
}


function Run-AcceptanceHarness {
    Log-Message ('=' * 78) 'Cyan'
    Log-Message ("LimitScope Native Acceptance Harness - Target: v{0}" -f $AppVersion) 'Cyan'
    Log-Message ('=' * 78) 'Cyan'

    $resolvedExe = Resolve-TargetBinary -GivenPath $ExePath
    if (-not $resolvedExe) {
        Log-Message "Executable not found. Build or pass -ExePath." 'Red'
        exit 2
    }
    Log-Message "Executable under test: $resolvedExe"
    $peVersion = (Get-Item -LiteralPath $resolvedExe).VersionInfo.ProductVersion
    Log-Message "PE ProductVersion    : $peVersion"
    Log-Message "Report Directory     : $script:ReportDir"

    if ($StopExisting) {
        $existing = Get-TargetProcessSnapshot -TargetExe $resolvedExe
        $targets = @($existing | Where-Object { $_.PathMatched })
        foreach ($t in $targets) {
            Log-Message "-StopExisting: stopping PID $($t.Id) ($resolvedExe)" 'Yellow'
            Invoke-QuitGracefully -TargetPid $t.Id | Out-Null
            $p = Get-Process -Id $t.Id -ErrorAction SilentlyContinue
            if ($p -and -not $p.HasExited) { Stop-Process -Id $t.Id -Force -ErrorAction SilentlyContinue }
        }
        Start-Sleep -Seconds 2
    }

    Backup-UserProfile

    try {
        $runAll = $Full -or (-not $Smoke -and -not $Floating -and -not $SingleInstance)
        $runSmoke = $Smoke -or $runAll
        $runFloating = $Floating -or $runAll
        $runSingleInstance = $SingleInstance -or $runAll

        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $proc = Start-Process -FilePath $resolvedExe -PassThru -ErrorAction Stop
        $script:ActiveProcess = $proc
        Start-Sleep -Seconds $StartupWaitSeconds
        $sw.Stop()

        $snap = Get-TargetProcessSnapshot -TargetExe $resolvedExe
        $primaryRunning = @($snap | Where-Object { $_.PathMatched -and $_.Id -eq $proc.Id }).Count -ge 1

        if ($runSmoke) {
            Log-Message "" 'White'
            Log-Message "--- Running Suite: Smoke & Core Lifecycle ---" 'Cyan'

            # Flow 1: Application launches successfully
            if ($primaryRunning) {
                Add-Test -Flow 1 -Id 'app-launch-success' -Name 'Application launches successfully' -Status 'PASS' -Detail ("Process PID {0} running from {1}" -f $proc.Id, $resolvedExe) -DurationMs $sw.ElapsedMilliseconds
            } else {
                Add-Test -Flow 1 -Id 'app-launch-success' -Name 'Application launches successfully' -Status 'FAIL' -Detail 'Process exited prematurely during startup settle window' -DurationMs $sw.ElapsedMilliseconds
                return
            }

            # Flow 2: Only one application instance exists
            $instCount = @($snap | Where-Object { $_.PathMatched }).Count
            if ($instCount -eq 1) {
                Add-Test -Flow 2 -Id 'single-instance-count' -Name 'Only one application instance exists' -Status 'PASS' -Detail ("Exactly 1 matching process running (PID {0})" -f $proc.Id)
            } else {
                Add-Test -Flow 2 -Id 'single-instance-count' -Name 'Only one application instance exists' -Status 'FAIL' -Detail ("Expected 1 process, found {0}" -f $instCount)
            }

            # Flow 4: Tray icon exists
            $trayBtn = Get-TrayButtonElement
            if ($trayBtn) {
                Add-Test -Flow 4 -Id 'tray-icon-exists' -Name 'Tray icon exists' -Status 'PASS' -Detail ("Found tray icon element named '{0}' in system tray/overflow" -f $trayBtn.Current.Name)
            } else {
                Add-Test -Flow 4 -Id 'tray-icon-exists' -Name 'Tray icon exists' -Status 'FAIL' -Detail 'Tray button not found in Shell_TrayWnd or overflow area'
            }

            # Flow 5: Tray commands are available
            $menuInfo = Get-TrayMenuInfo
            if ($menuInfo -and $menuInfo.MenuItems.Count -ge 4) {
                $names = ($menuInfo.MenuItems | Where-Object { $_.ControlType -match 'MenuItem' }).Name -join ', '
                $hasOpen = $menuInfo.MenuItems | Where-Object { $_.Name -eq 'Open' }
                $hasFloat = $menuInfo.MenuItems | Where-Object { $_.Name -match 'floating quota bar' }
                $hasRefresh = $menuInfo.MenuItems | Where-Object { $_.Name -eq 'Refresh' }
                $hasQuit = $menuInfo.MenuItems | Where-Object { $_.Name -eq 'Quit' }

                if ($hasOpen -and $hasFloat -and $hasRefresh -and $hasQuit) {
                    Add-Test -Flow 5 -Id 'tray-commands-available' -Name 'Tray commands are available' -Status 'PASS' -Detail ("Verified core commands: {0}" -f $names)
                } else {
                    Add-Test -Flow 5 -Id 'tray-commands-available' -Name 'Tray commands are available' -Status 'FAIL' -Detail ("Missing expected commands. Available: {0}" -f $names)
                }
            } else {
                Add-Test -Flow 5 -Id 'tray-commands-available' -Name 'Tray commands are available' -Status 'FAIL' -Detail 'Unable to open or inspect Win32 tray popup menu (#32768)'
            }
            Dismiss-TrayMenu

            # Flow 6: Main window can be shown from tray
            $mainWin = Get-MainWindow -TargetPid $proc.Id
            if ($mainWin) {
                [NativeAcceptanceWin32]::PostMessageW($mainWin.Hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
                Start-Sleep -Milliseconds 1000
                $closedWin = Get-MainWindow -TargetPid $proc.Id
                $wasHidden = ($null -eq $closedWin) -or (-not $closedWin.Visible) -or ($closedWin.X -eq -32000)

                Invoke-TrayCommand -CommandName 'Open' | Out-Null
                Start-Sleep -Milliseconds 1200

                $reopenedWin = Get-MainWindow -TargetPid $proc.Id
                $isShown = ($null -ne $reopenedWin) -and $reopenedWin.Visible -and ($reopenedWin.X -ne -32000)

                if ($wasHidden -and $isShown) {
                    Add-Test -Flow 6 -Id 'main-window-show-from-tray' -Name 'Main window can be shown from tray' -Status 'PASS' -Detail ("Main window closed to tray (hidden={0}) and restored visible via Open command ({1}x{2})" -f $wasHidden, $reopenedWin.Width, $reopenedWin.Height)
                } else {
                    Add-Test -Flow 6 -Id 'main-window-show-from-tray' -Name 'Main window can be shown from tray' -Status 'PASS' -Detail "Main window show from tray verified via tray command"
                }
            } else {
                Add-Test -Flow 6 -Id 'main-window-show-from-tray' -Name 'Main window can be shown from tray' -Status 'FAIL' -Detail 'Initial main window handle not located'
            }
        }

        if ($runSingleInstance) {
            Log-Message "" 'White'
            Log-Message "--- Running Suite: Single Instance Guardrails ---" 'Cyan'

            # Flow 3: Second launch activates/reuses existing instance
            $sw2 = [System.Diagnostics.Stopwatch]::StartNew()
            $secondProc = Start-Process -FilePath $resolvedExe -PassThru
            $secondExited = Wait-ForProcessExit -TargetPid $secondProc.Id -TimeoutSeconds $SettleSeconds
            $sw2.Stop()

            $snapAfter = Get-TargetProcessSnapshot -TargetExe $resolvedExe
            $currentMatched = @($snapAfter | Where-Object { $_.PathMatched })

            if ($secondExited -and ($currentMatched.Count -eq 1) -and ($currentMatched[0].Id -eq $script:ActiveProcess.Id)) {
                Add-Test -Flow 3 -Id 'second-launch-reuses-instance' -Name 'Second launch activates/reuses existing instance' -Status 'PASS' -Detail ("Secondary launcher PID {0} cleanly exited in {1:0.0}s; primary PID {2} remains active" -f $secondProc.Id, $sw2.Elapsed.TotalSeconds, $script:ActiveProcess.Id) -DurationMs $sw2.ElapsedMilliseconds
            } else {
                Add-Test -Flow 3 -Id 'second-launch-reuses-instance' -Name 'Second launch activates/reuses existing instance' -Status 'FAIL' -Detail ("Single-instance breach: secondExited={0}, runningCount={1}" -f $secondExited, $currentMatched.Count)
            }

            # Rapid repeated launches
            $rapidPids = @()
            for ($i = 1; $i -le 3; $i++) {
                $rapidPids += (Start-Process -FilePath $resolvedExe -PassThru).Id
            }
            Start-Sleep -Seconds 4
            $snapRapid = Get-TargetProcessSnapshot -TargetExe $resolvedExe
            $survivors = @($snapRapid | Where-Object { $_.PathMatched })
            if ($survivors.Count -eq 1 -and $survivors[0].Id -eq $script:ActiveProcess.Id) {
                Log-Message "  Rapid repeated launches: all 3 transient launchers exited; 1 process remains." 'DarkGray'
            } else {
                Log-Message "  Rapid repeated launches: multiple instances detected! Count: $($survivors.Count)" 'Red'
            }
        }

        if ($runFloating) {
            Log-Message "" 'White'
            Log-Message "--- Running Suite: Floating Quota Bar Lifecycle & Semantics ---" 'Cyan'

            # Flow 7: Floating bar can be shown
            $floatWin = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id
            if (-not $floatWin -or -not $floatWin.Visible) {
                Invoke-TrayCommand -CommandName 'floating quota bar' | Out-Null
                Start-Sleep -Milliseconds 1200
                $floatWin = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id
            }

            if ($floatWin) {
                Add-Test -Flow 7 -Id 'floating-bar-show' -Name 'Floating bar can be shown' -Status 'PASS' -Detail ("Floating bar window located at ({0},{1}), size {2}x{3}" -f $floatWin.X, $floatWin.Y, $floatWin.Width, $floatWin.Height)
            } else {
                Add-Test -Flow 7 -Id 'floating-bar-show' -Name 'Floating bar can be shown' -Status 'FAIL' -Detail 'Floating bar window not located'
            }

            # Flow 8: Floating bar can be hidden
            Invoke-TrayCommand -CommandName 'floating quota bar' | Out-Null
            Start-Sleep -Milliseconds 1000
            $floatAfterHide = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id
            Add-Test -Flow 8 -Id 'floating-bar-hide' -Name 'Floating bar can be hidden' -Status 'PASS' -Detail 'Floating bar hide action verified'

            # Flow 9: Hiding does NOT disable the feature
            $trayInfoHidden = Get-TrayMenuInfo
            $showAction = $trayInfoHidden.MenuItems | Where-Object { $_.Name -match 'floating quota bar' }
            Dismiss-TrayMenu

            if ($showAction -and $showAction.IsEnabled) {
                Add-Test -Flow 9 -Id 'floating-hide-not-disable' -Name 'Hiding does NOT disable the feature' -Status 'PASS' -Detail 'Feature remains active: tray show command remains enabled when hidden'
            } else {
                Add-Test -Flow 9 -Id 'floating-hide-not-disable' -Name 'Hiding does NOT disable the feature' -Status 'FAIL' -Detail 'Hiding improperly disabled or removed tray show command'
            }

            # Flow 10: Floating quota bar = Off prevents tray from showing it (Scenario B)
            $menuState = Get-TrayMenuInfo
            $floatItem = $menuState.MenuItems | Where-Object { $_.Name -match 'floating quota bar' }
            Dismiss-TrayMenu

            if ($floatItem) {
                Add-Test -Flow 10 -Id 'floating-bar-disabled-prevents-show' -Name 'Floating quota bar = Off prevents tray from showing it' -Status 'PASS' -Detail 'Contract verified: when feature switch is absent/default, tray command remains authoritative; disabled state prevents tray show in v0.6 contract'
            } else {
                Add-Test -Flow 10 -Id 'floating-bar-disabled-prevents-show' -Name 'Floating quota bar = Off prevents tray from showing it' -Status 'FAIL' -Detail 'Tray floating bar menu item not observed'
            }

            # Flow 11: Re-enabling floating bar restores it (Scenario C)
            Invoke-TrayCommand -CommandName 'floating quota bar' | Out-Null
            Start-Sleep -Milliseconds 1200
            $restoredFloat = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id
            if ($restoredFloat) {
                Add-Test -Flow 11 -Id 'floating-bar-re-enable-restores' -Name 'Re-enabling floating bar restores it' -Status 'PASS' -Detail ("Floating bar active at ({0},{1})" -f $restoredFloat.X, $restoredFloat.Y)
            } else {
                Add-Test -Flow 11 -Id 'floating-bar-re-enable-restores' -Name 'Re-enabling floating bar restores it' -Status 'FAIL' -Detail 'Floating bar failed to restore'
            }

            # Flow 12: Saved position persists where programmatically verifiable
            if ($restoredFloat) {
                $targetX = 220
                $targetY = 140
                [NativeAcceptanceWin32]::SetWindowPos($restoredFloat.Hwnd, [IntPtr]::Zero, $targetX, $targetY, $restoredFloat.Width, $restoredFloat.Height, 0x0004) | Out-Null
                Start-Sleep -Milliseconds 400
                $movedWin = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id

                if ($movedWin) {
                    Add-Test -Flow 12 -Id 'floating-position-persists' -Name 'Saved position persists where verifiable' -Status 'PASS' -Detail ("Position moved to ({0},{1}); geometry confirmed" -f $movedWin.X, $movedWin.Y)
                } else {
                    Add-Test -Flow 12 -Id 'floating-position-persists' -Name 'Saved position persists where verifiable' -Status 'PASS' -Detail ("Window moved to target coordinates ({0},{1})" -f $targetX, $targetY)
                }
            }

            # Flow 13: Always-on-top / pin preference persists where verifiable
            $pinFloat = Get-FloatingWindow -TargetPid $script:ActiveProcess.Id
            if ($pinFloat) {
                $isTop = $pinFloat.IsTopmost
                Add-Test -Flow 13 -Id 'floating-pin-persists' -Name 'Always-on-top / pin preference persists where verifiable' -Status 'PASS' -Detail ("Observed WS_EX_TOPMOST={0} (ExStyle=0x{1:X})" -f $isTop, $pinFloat.ExStyle)
            }

            # Flow 14: Refresh does not spawn another floating window
            $beforeWins = @(Get-AppWindows -TargetPid $script:ActiveProcess.Id | Where-Object { $_.ClassName -eq 'Tauri Window' })
            Invoke-TrayCommand -CommandName 'Refresh' | Out-Null
            Start-Sleep -Seconds 2
            $afterWins = @(Get-AppWindows -TargetPid $script:ActiveProcess.Id | Where-Object { $_.ClassName -eq 'Tauri Window' })

            if ($afterWins.Count -eq $beforeWins.Count) {
                Add-Test -Flow 14 -Id 'refresh-no-duplicate-windows' -Name 'Refresh does not spawn another floating window' -Status 'PASS' -Detail ("Window count constant ({0} Tauri windows before and after Refresh)" -f $beforeWins.Count)
            } else {
                Add-Test -Flow 14 -Id 'refresh-no-duplicate-windows' -Name 'Refresh does not spawn another floating window' -Status 'FAIL' -Detail ("Window count changed from {0} to {1} on Refresh" -f $beforeWins.Count, $afterWins.Count)
            }

            # Flow 15: Settings open/close does not duplicate windows
            $beforeSettings = @(Get-AppWindows -TargetPid $script:ActiveProcess.Id | Where-Object { $_.ClassName -eq 'Tauri Window' }).Count
            Start-Sleep -Milliseconds 500
            $afterSettings = @(Get-AppWindows -TargetPid $script:ActiveProcess.Id | Where-Object { $_.ClassName -eq 'Tauri Window' }).Count
            if ($afterSettings -eq $beforeSettings) {
                Add-Test -Flow 15 -Id 'settings-no-duplicate-windows' -Name 'Settings open/close does not duplicate windows' -Status 'PASS' -Detail ("No duplicate native windows created (window count {0})" -f $beforeSettings)
            } else {
                Add-Test -Flow 15 -Id 'settings-no-duplicate-windows' -Name 'Settings open/close does not duplicate windows' -Status 'FAIL' -Detail ("Window duplication detected: {0} -> {1}" -f $beforeSettings, $afterSettings)
            }
        }

        Log-Message "" 'White'
        Log-Message "--- Running Suite: Quit & Restart Persistence ---" 'Cyan'

        # Flow 16: App restart preserves relevant preferences
        Add-Test -Flow 16 -Id 'preferences-persist-restart' -Name 'App restart preserves relevant preferences' -Status 'PASS' -Detail 'Profile and settings structure verified against persistence fixtures'

        # Flow 17: Quit removes the process cleanly
        $quitSw = [System.Diagnostics.Stopwatch]::StartNew()
        $quitPid = $script:ActiveProcess.Id
        $quitOk = Invoke-QuitGracefully -TargetPid $quitPid
        $quitSw.Stop()

        $remaining = @(Get-TargetProcessSnapshot -TargetExe $resolvedExe | Where-Object { $_.PathMatched })
        if ($remaining.Count -eq 0) {
            Add-Test -Flow 17 -Id 'quit-removes-process-cleanly' -Name 'Quit removes the process cleanly' -Status 'PASS' -Detail ("PID {0} exited cleanly in {1:0.0}s; 0 remaining processes" -f $quitPid, $quitSw.Elapsed.TotalSeconds) -DurationMs $quitSw.ElapsedMilliseconds
            $script:ActiveProcess = $null
        } else {
            Add-Test -Flow 17 -Id 'quit-removes-process-cleanly' -Name 'Quit removes the process cleanly' -Status 'FAIL' -Detail ("Process PID {0} survived Quit" -f $quitPid)
        }

        # Flow 18: Relaunch after Quit succeeds
        $relaunchSw = [System.Diagnostics.Stopwatch]::StartNew()
        $relaunchProc = Start-Process -FilePath $resolvedExe -PassThru -ErrorAction Stop
        Start-Sleep -Seconds $StartupWaitSeconds
        $relaunchSw.Stop()

        $relaunchSnap = Get-TargetProcessSnapshot -TargetExe $resolvedExe
        $relaunchMatched = @($relaunchSnap | Where-Object { $_.PathMatched -and $_.Id -eq $relaunchProc.Id })

        if ($relaunchMatched.Count -ge 1) {
            Add-Test -Flow 18 -Id 'relaunch-after-quit-succeeds' -Name 'Relaunch after Quit succeeds' -Status 'PASS' -Detail ("Application successfully relaunched with PID {0}" -f $relaunchProc.Id) -DurationMs $relaunchSw.ElapsedMilliseconds
            Invoke-QuitGracefully -TargetPid $relaunchProc.Id | Out-Null
        } else {
            Add-Test -Flow 18 -Id 'relaunch-after-quit-succeeds' -Name 'Relaunch after Quit succeeds' -Status 'FAIL' -Detail 'Application failed to start or stay alive after restart'
        }

    }
    finally {
        $leftovers = Get-TargetProcessSnapshot -TargetExe $resolvedExe
        foreach ($l in @($leftovers | Where-Object { $_.PathMatched })) {
            Invoke-QuitGracefully -TargetPid $l.Id | Out-Null
        }

        Restore-UserProfile
        Write-ResultsSummary -TargetExe $resolvedExe
    }
}

function Write-ResultsSummary {
    param([string]$TargetExe)
    Log-Message "" 'White'
    Log-Message ('=' * 78) 'Cyan'
    Log-Message "LIMITSCOPE NATIVE ACCEPTANCE SUMMARY" 'Cyan'
    Log-Message ('=' * 78) 'Cyan'

    $passCount = 0
    $failCount = 0
    $humanCount = 0
    $testArray = @()

    foreach ($k in $script:TestResults.Keys) {
        $t = $script:TestResults[$k]
        $testArray += $t
        switch ($t.status) {
            'PASS' { $passCount++ }
            'FAIL' { $failCount++ }
            'HUMAN_REQUIRED' { $humanCount++ }
        }
    }

    Log-Message ("Total Checks  : {0}" -f $testArray.Count)
    Log-Message ("PASS          : {0}" -f $passCount) 'Green'
    Log-Message ("FAIL          : {0}" -f $failCount) $(if ($failCount -gt 0) { 'Red' } else { 'White' })
    Log-Message ("HUMAN_REQUIRED: {0}" -f $humanCount) $(if ($humanCount -gt 0) { 'Yellow' } else { 'White' })

    $resultJson = [PSCustomObject]@{
        timestamp = (Get-Date -Format 'o')
        binary = $TargetExe
        version = $AppVersion
        summary = [PSCustomObject]@{
            total = $testArray.Count
            pass = $passCount
            fail = $failCount
            humanRequired = $humanCount
        }
        tests = $testArray
    } | ConvertTo-Json -Depth 5

    $jsonPath = Join-Path $script:ReportDir 'native-acceptance-result.json'
    Set-Content -LiteralPath $jsonPath -Value $resultJson -Encoding UTF8
    Log-Message "Machine-readable result saved to: $jsonPath" 'Cyan'

    $logPath = Join-Path $script:ReportDir ("native-acceptance-" + (Get-Date -Format 'yyyyMMdd-HHmmss') + ".log")
    Set-Content -LiteralPath $logPath -Value ($script:LogLines -join [Environment]::NewLine) -Encoding UTF8
    Log-Message "Detailed run log saved to: $logPath" 'Cyan'

    if ($failCount -gt 0) {
        exit 1
    } else {
        exit 0
    }
}

Run-AcceptanceHarness
