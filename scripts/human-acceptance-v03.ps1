#Requires -Version 5.1
<#
.SYNOPSIS
  Safe, non-destructive Windows helper for the Rate Limits Human Acceptance
  pass (version-parameterized via -AppVersion).

.DESCRIPTION
  Automates only what is safely automatable - executable metadata, SHA-256,
  process lifecycle (launch, single instance, close-to-tray alive check, quit
  verification, restart relaunch) - and keeps every judgment call (tray icon,
  UI correctness, theme, countdown, predictions) with the human operator.
  Writes a timestamped text report.

  SAFETY CONTRACT (must hold at all times):
    - Never modifies provider credentials, auth files, or provider files.
    - Never edits registry startup entries or autostart settings.
    - Never installs, uninstalls, or executes an installer (inspection is
      read-only: existence, size, SHA-256, PE header, version metadata).
    - Never deletes or modifies application data, including localStorage.
    - Never kills unrelated processes. The only kill path is the explicit
      operator opt-in -StopExisting, which can only stop processes whose
      image path exactly matches -ExePath. Same-name processes with a
      different or unreadable image path are never touched.
    - The quit check never kills anything: if the process survives Quit,
      the script records FAIL evidence and leaves the process alone.

.PARAMETER ExePath
  Path to the Rate Limits executable, e.g.
  C:\AI\Token_Monitor\src-tauri\target\release\rate-limits.exe or the
  installed binary. Required unless the run is installer-inspection only.

.PARAMETER InstallerPath
  Optional path to an installer (e.g. Rate Limits_<version>_x64-setup.exe) for
  read-only inspection. The installer is never executed.

.PARAMETER ProcessName
  Process name used for detection. Default: the executable file name without
  extension (rate-limits.exe -> "rate-limits"). An installed build may use a
  different image name; the pre-flight section reports both name-matched and
  path-matched counts so mismatches are visible, and -ProcessName overrides.

.PARAMETER AppDisplayName
  Human-facing name used in prompts and the report. Default: "Rate Limits".

.PARAMETER AppVersion
  Expected application version for this acceptance pass. Default: "0.3.1".
  It labels the report header, the report filename, and the checklist
  document the report points to, and it is the expected value for the
  deterministic stale-binary guard: the pre-flight compares the executable's
  and installer's File/Product version against it and records
  exe-version-matches-appversion / installer-version-matches-appversion as
  PASS, as FAIL on a mismatch (STOP per the checklist section 1 pre-flight
  gate), or as NOT OBSERVED when the version metadata is missing or not
  numeric. The SHA-256 is always printed as provenance for the operator.

.PARAMETER ReportDir
  Output directory for the timestamped report. Default: <repo root>\artifacts
  (git-ignored; keep personal acceptance reports out of commits anyway, or
  pass a -ReportDir outside the repo if preferred).

.PARAMETER Phases
  Comma-separated subset of phases: Preflight, Launch, SingleInstance,
  CloseTray, Quit, Restart. They always run in canonical order regardless of
  the order given. Default: all. (A single string with commas keeps the
  parameter usable through "powershell -File" from cmd, bash, and PowerShell.)

.PARAMETER PreflightOnly
  Shorthand for -Phases Preflight (file/process report, no launch).

.PARAMETER StopExisting
  OPT-IN cleanup: before Launch, stop running instances whose image path
  exactly matches -ExePath (after listing them). Never touches same-name
  processes with a different or unreadable image path.

.PARAMETER StartupWaitSeconds
  Seconds to wait after a launch before the liveness check. Default 8.

.PARAMETER SettleSeconds
  Seconds to wait after the second launch in the single-instance check, so
  transient launcher stubs have time to exit. Default 15.

.PARAMETER ManualTimeoutSeconds
  Timeout for the manual wait prompts (close-to-tray, quit). After a timeout
  the script verifies the state anyway and marks the prompt as timed out.
  Default 900.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\human-acceptance-v03.ps1 `
    -ExePath "C:\AI\Token_Monitor\src-tauri\target\release\rate-limits.exe"

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\human-acceptance-v03.ps1 `
    -ExePath "C:\Program Files\Rate Limits\rate-limits.exe" `
    -InstallerPath "$env:USERPROFILE\Downloads\Rate Limits_0.3.1_x64-setup.exe" `
    -AppVersion 0.3.1

.EXAMPLE
  # Re-run only the quit verification against an already-running instance
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\human-acceptance-v03.ps1 `
    -ExePath "...\rate-limits.exe" -Phases Quit
#>
[CmdletBinding()]
param(
    [string]$ExePath,
    [string]$InstallerPath,
    [string]$ProcessName,
    [string]$AppDisplayName = 'Rate Limits',
    [string]$AppVersion = '0.3.1',
    [string]$ReportDir,
    [string]$Phases = 'Preflight,Launch,SingleInstance,CloseTray,Quit,Restart',
    [switch]$PreflightOnly,
    [switch]$StopExisting,
    [int]$StartupWaitSeconds = 8,
    [int]$SettleSeconds = 15,
    [int]$ManualTimeoutSeconds = 900
)

# ---------------------------------------------------------------------------
# Script-scope state
# ---------------------------------------------------------------------------
$script:AppDisplayName = $AppDisplayName
$script:AppVersion     = $AppVersion
$script:StopExisting   = $StopExisting
$script:StartupWaitSeconds = $StartupWaitSeconds
$script:SettleSeconds  = $SettleSeconds
$script:ManualTimeoutSeconds = $ManualTimeoutSeconds
$script:Report = New-Object 'System.Collections.Generic.List[string]'
$script:Checks = New-Object 'System.Collections.Specialized.OrderedDictionary'
$script:Aborted = $false
$script:InputEnded = $false
$script:LastPollCount = -1
$script:RunStartedAt = Get-Date
$script:OperatorConfirmedIds = @('restart-theme-persisted', 'restart-history-persisted')
$script:PhaseCheckId = @{
    'Preflight'     = 'no-preexisting-instance'
    'Launch'        = 'launch-alive-after-startup'
    'SingleInstance' = 'single-instance'
    'CloseTray'     = 'close-to-tray-process-alive'
    'Quit'          = 'quit-process-exited'
    'Restart'       = 'restart-relaunch'
}

# ---------------------------------------------------------------------------
# Output helpers: every line goes to the console AND into the report
# ---------------------------------------------------------------------------
function Write-ReportLine {
    param([string]$Text, [string]$Color = 'White')
    Write-Host $Text -ForegroundColor $Color
    $script:Report.Add($Text) | Out-Null
}

function Add-Check {
    param(
        [ValidateSet('PASS', 'FAIL', 'NOT OBSERVED')][string]$Result,
        [string]$Id,
        [string]$Detail
    )
    if ($script:Checks.Contains($Id)) { $script:Checks.Remove($Id) | Out-Null }
    $script:Checks[$Id] = @{ Result = $Result; Detail = $Detail }
    $color = switch ($Result) {
        'PASS' { 'Green' }
        'FAIL' { 'Red' }
        default { 'Yellow' }
    }
    Write-ReportLine ("  [{0}] {1} - {2}" -f $Result, $Id, $Detail) $color
}

# ---------------------------------------------------------------------------
# File facts / PE header (read-only)
# ---------------------------------------------------------------------------
function Get-FileFacts {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path
    $sha = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    return New-Object psobject -Property @{
        FullName        = $item.FullName
        SizeBytes       = $item.Length
        Sha256          = $sha
        FileVersion     = $item.VersionInfo.FileVersion
        ProductVersion  = $item.VersionInfo.ProductVersion
        ProductName     = $item.VersionInfo.ProductName
        FileDescription = $item.VersionInfo.FileDescription
        CompanyName     = $item.VersionInfo.CompanyName
    }
}

function Get-PeInfo {
    param([string]$Path)
    $result = @{ IsPe = $false; Machine = ''; Description = '' }
    try {
        $fs = [System.IO.File]::OpenRead($Path)
        try {
            $br = New-Object System.IO.BinaryReader($fs)
            if ($fs.Length -lt 64) { $result.Description = 'file too small to be a PE'; return $result }
            $dos = $br.ReadBytes(2)
            if (-not ($dos[0] -eq 0x4D -and $dos[1] -eq 0x5A)) {
                $result.Description = 'no MZ signature (not a PE)'; return $result
            }
            $null = $fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin)
            $peOffset = $br.ReadInt32()
            if ($peOffset -le 0 -or $peOffset -ge ($fs.Length - 6)) {
                $result.Description = 'invalid PE header offset'; return $result
            }
            $null = $fs.Seek($peOffset, [System.IO.SeekOrigin]::Begin)
            $sig = $br.ReadBytes(4)
            if (-not ($sig[0] -eq 0x50 -and $sig[1] -eq 0x45 -and $sig[2] -eq 0 -and $sig[3] -eq 0)) {
                $result.Description = 'no PE\0\0 signature'; return $result
            }
            $machine = $br.ReadUInt16()
            $result.IsPe = $true
            switch ($machine) {
                0x014C { $result.Machine = 'x86' }
                0x8664 { $result.Machine = 'x64 (AMD64)' }
                0xAA64 { $result.Machine = 'ARM64' }
                0x01C4 { $result.Machine = 'ARM Thumb-2' }
                default { $result.Machine = ('0x{0:X4}' -f $machine) }
            }
            $result.Description = ('valid PE header, machine={0}' -f $result.Machine)
            return $result
        }
        finally {
            $br.Close()
            $fs.Dispose()
        }
    }
    catch {
        $result.Description = ('PE inspection failed: ' + $_.Exception.Message)
        return $result
    }
}

function Test-VersionMatches {
    # Deterministic stale-binary guard (v0.3.0 lesson: a stale v0.2.0 binary
    # was caught in pre-flight). Returns $true when $Actual's leading numeric
    # version components equal $Expected (extra trailing components allowed,
    # e.g. "0.3.1.0" vs "0.3.1"), $false on a leading-component mismatch
    # (e.g. "0.2.0"), and $null when either value is missing or not fully
    # numeric (caller records NOT OBSERVED). Evidence only: the helper never
    # takes any action based on the result; the operator decides.
    param([string]$Actual, [string]$Expected)
    if ([string]::IsNullOrWhiteSpace($Actual) -or [string]::IsNullOrWhiteSpace($Expected)) { return $null }
    $actualParts = @($Actual.Trim() -split '\.')
    $expectedParts = @($Expected.Trim() -split '\.')
    if ($expectedParts.Count -gt $actualParts.Count) { return $false }
    for ($i = 0; $i -lt $expectedParts.Count; $i++) {
        $a = 0L
        $e = 0L
        if (-not [Int64]::TryParse($actualParts[$i], [ref]$a)) { return $null }
        if (-not [Int64]::TryParse($expectedParts[$i], [ref]$e)) { return $null }
        if ($a -ne $e) { return $false }
    }
    return $true
}

# ---------------------------------------------------------------------------
# Process detection
# ---------------------------------------------------------------------------
function Get-AppSnapshot {
    # Detection logic (documented):
    #   1. Candidates are processes whose image name equals -ProcessName
    #      (default: the target executable file name without extension).
    #   2. For each candidate we try to read the image path. PathMatched is
    #      true only when the image path is readable AND equals the target
    #      executable path (case-insensitive).
    #   3. Transient launcher stubs are not assumed away: verdicts are made
    #      only after a settle window (see SettleSeconds), and the started
    #      PID's exit status is reported separately from surviving processes.
    if ([string]::IsNullOrEmpty($script:ProcessName)) { return ,@() }
    $escaped = [System.Management.Automation.WildcardPattern]::Escape($script:ProcessName)
    $procs = @(Get-Process -Name $escaped -ErrorAction SilentlyContinue)
    $out = @()
    foreach ($p in $procs) {
        $path = $null
        try { $path = $p.Path } catch { }
        if ([string]::IsNullOrEmpty($path)) {
            try { if ($null -ne $p.MainModule) { $path = $p.MainModule.FileName } } catch { }
        }
        $start = $null
        try { $start = $p.StartTime } catch { }
        $title = $null
        try { $title = $p.MainWindowTitle } catch { }
        $matched = $false
        if (-not [string]::IsNullOrEmpty($path) -and -not [string]::IsNullOrEmpty($script:ExePath)) {
            $matched = ([string]::Compare($path, $script:ExePath, $true) -eq 0)
        }
        $out += New-Object psobject -Property @{
            Id              = $p.Id
            ProcessName     = $p.ProcessName
            StartTime       = $start
            ImagePath       = $path
            PathMatched     = $matched
            MainWindowTitle = $title
        }
    }
    return ,$out
}

function Get-CountInfo {
    # Verdict-counting rule (documented):
    #   - If every candidate has a readable image path, only path-matched
    #     candidates count. This protects against an unrelated same-named
    #     binary elsewhere on the machine.
    #   - If any candidate's image path is unreadable (e.g. elevated
    #     process), fall back to the name-matched count and flag the
    #     reduced confidence in the report.
    param($Snapshot)
    $snapshot = @($Snapshot)
    $total = $snapshot.Count
    $matched = @($snapshot | Where-Object { $_.PathMatched }).Count
    $allReadable = $true
    foreach ($s in $snapshot) {
        if ([string]::IsNullOrEmpty($s.ImagePath)) { $allReadable = $false }
    }
    $count = $matched
    $mode = 'path-matched (authoritative)'
    if ($total -gt 0 -and -not $allReadable) {
        $count = $total
        $mode = 'name-matched (some image paths unreadable - lower confidence)'
    }
    return New-Object psobject -Property @{
        RunningCount     = $count
        NameMatchedTotal = $total
        PathMatchedCount = $matched
        AllPathsReadable = $allReadable
        Mode             = $mode
    }
}

function Format-SnapshotEvidence {
    param($Snapshot)
    $lines = @()
    foreach ($s in @($Snapshot)) {
        $start = if ($null -ne $s.StartTime) { $s.StartTime.ToString('yyyy-MM-dd HH:mm:ss') } else { 'unknown' }
        $path = if ($s.ImagePath) { $s.ImagePath } else { '(image path unreadable)' }
        $title = if ($s.MainWindowTitle) { ('  window="{0}"' -f $s.MainWindowTitle) } else { '' }
        $lines += ('    PID {0}  started {1}  path={2}{3}' -f $s.Id, $start, $path, $title)
    }
    return $lines
}

function Write-SnapshotEvidence {
    param($Snapshot)
    $lines = Format-SnapshotEvidence -Snapshot $Snapshot
    foreach ($line in $lines) { Write-ReportLine $line 'Gray' }
    if (@($Snapshot).Count -eq 0) { Write-ReportLine '    (no matching processes)' 'Gray' }
}

# ---------------------------------------------------------------------------
# Operator interaction
# ---------------------------------------------------------------------------
function Wait-Operator {
    # Interactive wait used for manual actions. Polls the process state and
    # prints a line whenever the count changes; Enter ends the wait. After a
    # timeout the caller verifies the state anyway (never a silent skip).
    param([string[]]$PromptLines, [int]$TimeoutSeconds)
    Write-ReportLine '' 'White'
    foreach ($line in $PromptLines) { Write-ReportLine $line 'Cyan' }
    if ([Console]::IsInputRedirected) {
        $null = Read-Host '  Press Enter when done'
        return 'enter'
    }
    try {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $script:LastPollCount = -1
        $lastBeat = -1
        while ($true) {
            if ([Console]::KeyAvailable) {
                $key = [Console]::ReadKey($true)
                if ($key.Key -eq [ConsoleKey]::Enter) { return 'enter' }
                # other keys are consumed and ignored
            }
            $info = Get-CountInfo -Snapshot (Get-AppSnapshot)
            if ($info.RunningCount -ne $script:LastPollCount) {
                Write-ReportLine ('  [poll] {0} process(es) now: {1}' -f $script:AppDisplayName, $info.RunningCount) 'DarkGray'
                $script:LastPollCount = $info.RunningCount
                $lastBeat = [int]$sw.Elapsed.TotalSeconds
            }
            else {
                $elapsed = [int]$sw.Elapsed.TotalSeconds
                if ($elapsed - $lastBeat -ge 30) {
                    Write-ReportLine ('  [poll] waiting... process(es): {0} ({1:0}s elapsed, timeout {2}s)' -f $info.RunningCount, $elapsed, $TimeoutSeconds) 'DarkGray'
                    $lastBeat = $elapsed
                }
            }
            if ($TimeoutSeconds -gt 0 -and $sw.Elapsed.TotalSeconds -ge $TimeoutSeconds) { return 'timeout' }
            Start-Sleep -Milliseconds 500
        }
    }
    catch {
        $null = Read-Host '  Press Enter when done'
        return 'enter'
    }
}

function Get-OperatorChoice {
    # Single-token choice via Read-Host (works interactively and with piped
    # input). Three consecutive empty reads (EOF / abandoned console) yield
    # the safe default and disable further prompting.
    param([string]$Prompt, [string[]]$Valid, [string]$DefaultOnEof)
    if ($script:InputEnded) { return $DefaultOnEof }
    $empty = 0
    while ($true) {
        $answer = Read-Host $Prompt
        if ($null -ne $answer) { $answer = $answer.Trim() }
        if (-not [string]::IsNullOrEmpty($answer)) {
            foreach ($v in $Valid) { if ($answer -ieq $v) { return $v } }
            Write-ReportLine ("  Unrecognized answer '{0}'. Expected one of: {1}" -f $answer, ($Valid -join '/')) 'Yellow'
            continue
        }
        $empty++
        if ($empty -ge 3) { break }
    }
    Write-ReportLine '  (no operator input received - using safe default)' 'Yellow'
    $script:InputEnded = $true
    return $DefaultOnEof
}

function Invoke-StopExisting {
    # The ONLY kill path in this script. Requires the operator to have passed
    # -StopExisting explicitly, and only stops processes whose image path
    # exactly equals the target executable path.
    Write-ReportLine '  -StopExisting: operator opt-in cleanup requested.' 'Yellow'
    $snap = Get-AppSnapshot
    $targets = @($snap | Where-Object { $_.PathMatched })
    $skipped = @($snap | Where-Object { -not $_.PathMatched })
    foreach ($t in $targets) {
        Write-ReportLine ("  -StopExisting: stopping PID {0} (exact path match: {1})" -f $t.Id, $t.ImagePath) 'Yellow'
        try { Stop-Process -Id $t.Id -Force -ErrorAction Stop } catch {
            Write-ReportLine ("    failed to stop PID {0}: {1}" -f $t.Id, $_.Exception.Message) 'Red'
        }
    }
    foreach ($s in $skipped) {
        $p = if ($s.ImagePath) { $s.ImagePath } else { '(image path unreadable)' }
        Write-ReportLine ("  -StopExisting: NOT touching PID {0} (same name, different or unreadable path: {1})" -f $s.Id, $p) 'Yellow'
    }
    Start-Sleep -Seconds 2
    $after = Get-CountInfo -Snapshot (Get-AppSnapshot)
    if ($after.RunningCount -gt 0) {
        Write-ReportLine '  -StopExisting: matching processes still remain after cleanup.' 'Red'
        Write-SnapshotEvidence -Snapshot (Get-AppSnapshot)
        return $false
    }
    return $true
}

# ---------------------------------------------------------------------------
# Phases
# ---------------------------------------------------------------------------
function Invoke-PreflightPhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: PRE-FLIGHT  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    $facts = Get-FileFacts -Path $script:ExePath
    Write-ReportLine ("  Executable path    : {0}" -f $facts.FullName)
    Write-ReportLine ("  Size               : {0} bytes" -f $facts.SizeBytes)
    Write-ReportLine ("  SHA-256            : {0}" -f $facts.Sha256)
    $fv = if ($facts.FileVersion) { $facts.FileVersion } else { '(not set)' }
    $pv = if ($facts.ProductVersion) { $facts.ProductVersion } else { '(not set)' }
    $pn = if ($facts.ProductName) { $facts.ProductName } else { '(not set)' }
    Write-ReportLine ("  File version       : {0}" -f $fv)
    Write-ReportLine ("  Product version    : {0}" -f $pv)
    Write-ReportLine ("  Product name       : {0}" -f $pn)
    # Deterministic stale-binary guard (see checklist pre-flight STOP gate).
    $verActual = if ($facts.ProductVersion) { $facts.ProductVersion } else { $facts.FileVersion }
    $verField = if ($facts.ProductVersion) { 'ProductVersion' } elseif ($facts.FileVersion) { 'FileVersion' } else { '(no version metadata)' }
    $verMatch = Test-VersionMatches -Actual $verActual -Expected $script:AppVersion
    if ($null -eq $verMatch) {
        Add-Check -Id 'exe-version-matches-appversion' -Result 'NOT OBSERVED' `
            -Detail ("{0} '{1}' is missing or not numeric; compare against expected {2} via the SHA-256 provenance recorded in the checklist instead" -f $verField, $verActual, $script:AppVersion)
    }
    elseif ($verMatch) {
        Add-Check -Id 'exe-version-matches-appversion' -Result 'PASS' `
            -Detail ("{0} {1} matches expected version {2}" -f $verField, $verActual, $script:AppVersion)
    }
    else {
        Add-Check -Id 'exe-version-matches-appversion' -Result 'FAIL' `
            -Detail ("{0} '{1}' does not match expected version {2} - possible stale binary; STOP per the checklist pre-flight gate" -f $verField, $verActual, $script:AppVersion)
    }
    $pe = Get-PeInfo -Path $script:ExePath
    Write-ReportLine ("  PE header          : {0}" -f $pe.Description)

    $snap = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $snap
    Write-ReportLine ("  Process detection  : {0}" -f $info.Mode)
    Write-ReportLine ("  Name-matched count : {0}  (image name '{1}')" -f $info.NameMatchedTotal, $script:ProcessName)
    Write-ReportLine ("  Path-matched count : {0}  (image path equals target executable)" -f $info.PathMatchedCount)
    Write-ReportLine ("  Timestamp          : {0}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'))
    if ($snap.Count -gt 0) {
        Write-ReportLine '  Current matching processes:' 'Gray'
        Write-SnapshotEvidence -Snapshot $snap
    }
    if ($info.NameMatchedTotal -gt $info.PathMatchedCount) {
        Write-ReportLine '  NOTE: name-matched processes exist that do NOT have the target image path. They are' 'Yellow'
        Write-ReportLine '  treated as unrelated processes: never killed, and not counted as instances of the target.' 'Yellow'
    }

    if ($info.RunningCount -eq 0) {
        Add-Check -Id 'no-preexisting-instance' -Result 'PASS' -Detail 'no matching process running at pre-flight'
        return
    }

    Add-Check -Id 'no-preexisting-instance' -Result 'FAIL' -Detail ("{0} matching process(es) already running before the test" -f $info.RunningCount)
    Write-ReportLine ('  WARNING: {0} matching process(es) already running. The helper did NOT stop them.' -f $info.RunningCount) 'Yellow'

    if ($script:StopExisting) {
        $cleared = Invoke-StopExisting
        if (-not $cleared) {
            Write-ReportLine '  Aborting: cannot start from a clean state. Close the remaining instance manually.' 'Red'
            $script:Aborted = $true
            return
        }
        Add-Check -Id 'no-preexisting-instance' -Result 'PASS' -Detail 'cleared via operator opt-in -StopExisting (exact image-path match only)'
        return
    }

    $choice = Get-OperatorChoice '  Continue with the existing instance running (C) or abort (A)? [C/A, default A]' @('C', 'A') 'A'
    if ($choice -eq 'A') {
        Write-ReportLine '  Operator aborted. No launch was performed.' 'Yellow'
        $script:Aborted = $true
        return
    }
    Write-ReportLine '  [operator] chose to continue with the existing instance running; results may be tainted by it.' 'Yellow'
}

function Invoke-LaunchPhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: LAUNCH CHECK  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    $before = Get-CountInfo -Snapshot (Get-AppSnapshot)
    if ($before.RunningCount -gt 0) {
        Write-ReportLine ('  NOTE: {0} matching process(es) already exist before this launch; attribution below may be ambiguous.' -f $before.RunningCount) 'Yellow'
    }
    $started = $null
    try {
        $started = Start-Process -FilePath $script:ExePath `
            -WorkingDirectory (Split-Path -Parent $script:ExePath) `
            -PassThru -ErrorAction Stop
    }
    catch {
        Add-Check -Id 'launch-alive-after-startup' -Result 'FAIL' -Detail ('Start-Process failed: ' + $_.Exception.Message)
        return
    }
    $startTime = $null
    try { $startTime = $started.StartTime } catch { }
    $startText = if ($null -ne $startTime) { $startTime.ToString('yyyy-MM-dd HH:mm:ss') } else { 'unknown' }
    Write-ReportLine ("  Launched PID {0} at {1}; waiting {2}s for startup..." -f $started.Id, $startText, $script:StartupWaitSeconds)
    Start-Sleep -Seconds $script:StartupWaitSeconds

    $exited = $null
    $exitCode = $null
    try { $started.Refresh(); $exited = $started.HasExited } catch { $exited = $null }
    if ($exited) { try { $exitCode = $started.ExitCode } catch { } }
    $startedStatus = switch ($exited) {
        $true { if ($null -ne $exitCode) { "exited (code {0})" -f $exitCode } else { 'exited (exit code unavailable)' } }
        $false { 'still running' }
        default { 'status unavailable (possibly a launcher stub that handed off, or access denied)' }
    }
    Write-ReportLine ("  Started PID {0}: {1}" -f $started.Id, $startedStatus)

    $snap = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $snap
    Write-ReportLine ("  Long-lived matching processes after startup wait: {0} ({1})" -f $info.RunningCount, $info.Mode)
    Write-SnapshotEvidence -Snapshot $snap

    if ($info.RunningCount -ge 1) {
        Add-Check -Id 'launch-alive-after-startup' -Result 'PASS' `
            -Detail ("{0} long-lived process(es) remain {1}s after launch (started PID {2}: {3})" -f $info.RunningCount, $script:StartupWaitSeconds, $started.Id, $startedStatus)
        Write-ReportLine '  NOTE: the check counts long-lived app processes, not only the started PID. A launcher' 'Gray'
        Write-ReportLine '  stub that exits after spawning the real process does not fail this check by itself.' 'Gray'
    }
    else {
        Add-Check -Id 'launch-alive-after-startup' -Result 'FAIL' `
            -Detail ("no matching process remained {0}s after launch (started PID {1}: {2})" -f $script:StartupWaitSeconds, $started.Id, $startedStatus)
    }
}

function Invoke-SingleInstancePhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: SINGLE INSTANCE CHECK  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    $before = Get-AppSnapshot
    $beforeInfo = Get-CountInfo -Snapshot $before
    Write-ReportLine ("  Before second launch: {0} matching process(es)" -f $beforeInfo.RunningCount)
    if ($beforeInfo.RunningCount -eq 0) {
        Add-Check -Id 'single-instance' -Result 'FAIL' -Detail 'no existing instance before the second launch (launch phase did not leave one)'
        return
    }
    if ($beforeInfo.RunningCount -gt 1) {
        Write-ReportLine '  NOTE: more than one instance existed before the second launch; this result may be tainted.' 'Yellow'
    }

    $started = $null
    try {
        $started = Start-Process -FilePath $script:ExePath `
            -WorkingDirectory (Split-Path -Parent $script:ExePath) `
            -PassThru -ErrorAction Stop
    }
    catch {
        Add-Check -Id 'single-instance' -Result 'FAIL' -Detail ('second Start-Process failed: ' + $_.Exception.Message)
        return
    }
    Write-ReportLine ("  Second launch PID {0}; waiting {1}s settle window so transient launcher stubs can exit..." -f $started.Id, $script:SettleSeconds)
    Start-Sleep -Seconds $script:SettleSeconds

    $exited = $null
    try { $started.Refresh(); $exited = $started.HasExited } catch { $exited = $null }
    $secondStatus = switch ($exited) {
        $true { 'exited during settle window (expected for a single-instance guard or launcher stub)' }
        $false { 'still running' }
        default { 'status unavailable' }
    }
    Write-ReportLine ("  Second PID {0}: {1}" -f $started.Id, $secondStatus)

    $after = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $after
    Write-ReportLine ("  Long-lived matching processes after settle window: {0}" -f $info.RunningCount)
    Write-SnapshotEvidence -Snapshot $after

    if ($info.RunningCount -eq 1) {
        Add-Check -Id 'single-instance' -Result 'PASS' `
            -Detail ("exactly one long-lived process remains after the second launch (second PID {0}: {1})" -f $started.Id, $secondStatus)
    }
    elseif ($info.RunningCount -eq 0) {
        Add-Check -Id 'single-instance' -Result 'FAIL' -Detail 'no long-lived process remained after the second launch'
    }
    else {
        $pids = (@($after) | ForEach-Object { $_.Id }) -join ', '
        Add-Check -Id 'single-instance' -Result 'FAIL' -Detail ("{0} long-lived processes remain after the second launch (PIDs: {1})" -f $info.RunningCount, $pids)
        Write-ReportLine '  The helper did NOT kill the extra instance; the operator decides how to resolve it.' 'Yellow'
    }
}

function Invoke-CloseTrayPhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: CLOSE TO TRAY (MANUAL)  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    $result = Wait-Operator -TimeoutSeconds $script:ManualTimeoutSeconds -PromptLines @(
        '  MANUAL ACTION REQUIRED',
        '  Close the dashboard window manually now; the process should remain alive.',
        '  (Do not use tray > Quit yet. The helper polls the process state while you work.)',
        ('  Press Enter after you have closed the window. Timeout: {0}s.' -f $script:ManualTimeoutSeconds)
    )
    if ($result -eq 'timeout') {
        Write-ReportLine '  (prompt timed out - verifying the current state anyway)' 'Yellow'
    }
    Start-Sleep -Seconds 2
    $snap = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $snap
    Write-ReportLine ("  Matching processes after window close: {0}" -f $info.RunningCount)
    Write-SnapshotEvidence -Snapshot $snap
    if ($info.RunningCount -ge 1) {
        Add-Check -Id 'close-to-tray-process-alive' -Result 'PASS' `
            -Detail 'process still alive after the dashboard window was closed (mechanical check only - tray icon visibility is confirmed visually by the operator)'
    }
    else {
        Add-Check -Id 'close-to-tray-process-alive' -Result 'FAIL' -Detail 'no matching process remained after the dashboard window was closed (app appears to have quit instead of hiding)'
    }
}

function Invoke-QuitPhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: QUIT VIA TRAY (MANUAL)  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    Wait-Operator -TimeoutSeconds $script:ManualTimeoutSeconds -PromptLines @(
        '  MANUAL ACTION REQUIRED',
        '  Open the Rate Limits tray icon and choose Quit.',
        '  (The helper will never kill the process; it only verifies the outcome.)',
        ('  Press Enter after you have chosen Quit, or once the tray icon disappears. Timeout: {0}s.' -f $script:ManualTimeoutSeconds)
    ) | Out-Null
    Start-Sleep -Seconds 3
    $snap = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $snap
    Write-ReportLine ("  Matching processes after tray Quit: {0}" -f $info.RunningCount)
    Write-SnapshotEvidence -Snapshot $snap
    if ($info.RunningCount -eq 0) {
        Add-Check -Id 'quit-process-exited' -Result 'PASS' -Detail 'no matching process remains after tray Quit'
    }
    else {
        $pids = (@($snap) | ForEach-Object { $_.Id }) -join ', '
        Add-Check -Id 'quit-process-exited' -Result 'FAIL' `
            -Detail ("{0} matching process(es) still running after tray Quit (PIDs: {1}). The helper did NOT kill them; close them manually or rerun pre-flight with -StopExisting." -f $info.RunningCount, $pids)
    }
}

function Invoke-RestartPhase {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: RESTART PERSISTENCE (MANUAL STATE CHANGE + RELAUNCH)  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    $choice = Get-OperatorChoice '  Did you change theme/history state and want the helper to relaunch the app now? [Y/N, default N]' @('Y', 'N') 'N'
    if ($choice -ne 'Y') {
        Write-ReportLine '  [operator] restart phase declined.' 'Yellow'
        Add-Check -Id 'restart-relaunch' -Result 'NOT OBSERVED' -Detail 'operator declined the restart phase'
        Add-Check -Id 'restart-theme-persisted' -Result 'NOT OBSERVED' -Detail 'restart phase not run'
        Add-Check -Id 'restart-history-persisted' -Result 'NOT OBSERVED' -Detail 'restart phase not run'
        return
    }
    Write-ReportLine '  [operator] confirmed theme/history state was changed; restart lifecycle executed.' 'Yellow'

    $before = Get-CountInfo -Snapshot (Get-AppSnapshot)
    if ($before.RunningCount -gt 0) {
        Write-ReportLine ('  NOTE: {0} matching process(es) already running before the relaunch; result may be tainted.' -f $before.RunningCount) 'Yellow'
    }
    $started = $null
    try {
        $started = Start-Process -FilePath $script:ExePath `
            -WorkingDirectory (Split-Path -Parent $script:ExePath) `
            -PassThru -ErrorAction Stop
    }
    catch {
        Add-Check -Id 'restart-relaunch' -Result 'FAIL' -Detail ('relaunch Start-Process failed: ' + $_.Exception.Message)
        Add-Check -Id 'restart-theme-persisted' -Result 'NOT OBSERVED' -Detail 'relaunch failed'
        Add-Check -Id 'restart-history-persisted' -Result 'NOT OBSERVED' -Detail 'relaunch failed'
        return
    }
    Write-ReportLine ("  Relaunched PID {0}; waiting {1}s for startup..." -f $started.Id, $script:StartupWaitSeconds)
    Start-Sleep -Seconds $script:StartupWaitSeconds
    $snap = Get-AppSnapshot
    $info = Get-CountInfo -Snapshot $snap
    Write-SnapshotEvidence -Snapshot $snap
    if ($info.RunningCount -ge 1) {
        Add-Check -Id 'restart-relaunch' -Result 'PASS' -Detail 'restart lifecycle executed: state change confirmed by operator, app relaunched, process verified alive. Theme/history correctness is NOT asserted by the helper - see operator confirmations below.'
    }
    else {
        Add-Check -Id 'restart-relaunch' -Result 'FAIL' -Detail 'app did not stay alive after the restart relaunch'
        Add-Check -Id 'restart-theme-persisted' -Result 'NOT OBSERVED' -Detail 'app not running after relaunch'
        Add-Check -Id 'restart-history-persisted' -Result 'NOT OBSERVED' -Detail 'app not running after relaunch'
        return
    }

    $theme = Get-OperatorChoice '  Visually confirm: did the previously selected theme persist after restart? [Y=confirmed / N=not preserved / S=skip, default S]' @('Y', 'N', 'S') 'S'
    $themeResult = switch ($theme) {
        'Y' { 'PASS' }
        'N' { 'FAIL' }
        default { 'NOT OBSERVED' }
    }
    Write-ReportLine ("  [operator] theme persistence: {0}" -f $theme) 'Yellow'
    Add-Check -Id 'restart-theme-persisted' -Result $themeResult -Detail ('operator visual confirmation: ' + $theme)

    $hist = Get-OperatorChoice '  Visually confirm: did quota history collected before the restart persist? [Y/N/S, default S]' @('Y', 'N', 'S') 'S'
    $histResult = switch ($hist) {
        'Y' { 'PASS' }
        'N' { 'FAIL' }
        default { 'NOT OBSERVED' }
    }
    Write-ReportLine ("  [operator] history persistence: {0}" -f $hist) 'Yellow'
    Add-Check -Id 'restart-history-persisted' -Result $histResult -Detail ('operator visual confirmation: ' + $hist)
}

function Invoke-InstallerInspection {
    Write-ReportLine '' 'White'
    Write-ReportLine ('=== PHASE: INSTALLER INSPECTION (READ-ONLY)  ({0}) ===' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) 'Cyan'
    if (-not (Test-Path -LiteralPath $script:InstallerPath -PathType Leaf)) {
        Add-Check -Id 'installer-present' -Result 'FAIL' -Detail ("installer not found: {0}" -f $script:InstallerPath)
        return
    }
    $facts = Get-FileFacts -Path $script:InstallerPath
    Write-ReportLine ("  Installer path     : {0}" -f $facts.FullName)
    Write-ReportLine ("  Size               : {0} bytes" -f $facts.SizeBytes)
    Write-ReportLine ("  SHA-256            : {0}" -f $facts.Sha256)
    $fv = if ($facts.FileVersion) { $facts.FileVersion } else { '(not set)' }
    $pv = if ($facts.ProductVersion) { $facts.ProductVersion } else { '(not set)' }
    Write-ReportLine ("  File version       : {0}" -f $fv)
    Write-ReportLine ("  Product version    : {0}" -f $pv)
    # Deterministic stale-binary guard (see checklist pre-flight STOP gate).
    $insVerActual = if ($facts.ProductVersion) { $facts.ProductVersion } else { $facts.FileVersion }
    $insVerField = if ($facts.ProductVersion) { 'ProductVersion' } elseif ($facts.FileVersion) { 'FileVersion' } else { '(no version metadata)' }
    $insVerMatch = Test-VersionMatches -Actual $insVerActual -Expected $script:AppVersion
    if ($null -eq $insVerMatch) {
        Add-Check -Id 'installer-version-matches-appversion' -Result 'NOT OBSERVED' `
            -Detail ("{0} '{1}' is missing or not numeric; compare against expected {2} via the recorded SHA-256 instead" -f $insVerField, $insVerActual, $script:AppVersion)
    }
    elseif ($insVerMatch) {
        Add-Check -Id 'installer-version-matches-appversion' -Result 'PASS' `
            -Detail ("{0} {1} matches expected version {2}" -f $insVerField, $insVerActual, $script:AppVersion)
    }
    else {
        Add-Check -Id 'installer-version-matches-appversion' -Result 'FAIL' `
            -Detail ("{0} '{1}' does not match expected version {2} - possible stale installer; STOP per the checklist pre-flight gate" -f $insVerField, $insVerActual, $script:AppVersion)
    }
    $pe = Get-PeInfo -Path $script:InstallerPath
    Write-ReportLine ("  PE header          : {0}" -f $pe.Description)
    Add-Check -Id 'installer-present' -Result 'PASS' -Detail ("{0} ({1} bytes, version {2})" -f $facts.FullName, $facts.SizeBytes, $fv)
    if ($pe.IsPe) {
        Add-Check -Id 'installer-pe-header' -Result 'PASS' -Detail $pe.Description
    }
    else {
        Add-Check -Id 'installer-pe-header' -Result 'FAIL' -Detail $pe.Description
    }
    Write-ReportLine '  The installer was inspected read-only. It was NOT executed; installing/uninstalling' 'Gray'
    Write-ReportLine ('  remains a manual operator step (see docs/human-acceptance-v{0}.md section 8).' -f $script:AppVersion) 'Gray'
}

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
function Write-ChecksSummary {
    Write-ReportLine '' 'White'
    Write-ReportLine '=== AUTOMATED CHECK RESULTS ===' 'Cyan'
    $autoCount = 0
    $opAnswered = 0
    $autoFail = 0
    $autoPass = 0
    foreach ($id in $script:Checks.Keys) {
        $check = $script:Checks[$id]
        $color = switch ($check.Result) {
            'PASS' { 'Green' }
            'FAIL' { 'Red' }
            default { 'Yellow' }
        }
        Write-ReportLine ("  [{0}] {1} - {2}" -f $check.Result, $id, $check.Detail) $color
        if ($script:OperatorConfirmedIds -contains $id) {
            if ($check.Result -ne 'NOT OBSERVED') { $opAnswered++ }
        }
        else {
            $autoCount++
            if ($check.Result -eq 'PASS') { $autoPass++ }
            if ($check.Result -eq 'FAIL') { $autoFail++ }
        }
    }
    Write-ReportLine '' 'White'
    Write-ReportLine ("  Automated checks: {0} total, {1} PASS, {2} FAIL, {3} NOT OBSERVED" -f `
            $autoCount, $autoPass, $autoFail, ($autoCount - $autoPass - $autoFail)) 'White'
    if ($opAnswered -gt 0) {
        Write-ReportLine ('  Operator-confirmed answers recorded this run: {0} (marked above; the helper never asserts them on its own)' -f $opAnswered) 'White'
    }

    Write-ReportLine '' 'White'
    $overall = if ($autoFail -gt 0) { 'FAIL' }
        elseif ($autoPass -eq $autoCount -and $autoCount -gt 0) { 'PASS (mechanical checks only)' }
        else { 'NOT FULLY OBSERVED' }
    Write-ReportLine ("  AUTOMATED LIFECYCLE RESULT: {0}" -f $overall) $(if ($autoFail -gt 0) { 'Red' } else { 'Cyan' })
}

function Write-ManualChecklist {
    Write-ReportLine '' 'White'
    Write-ReportLine '=== MANUAL CHECKLIST (NOT automatable - NOT OBSERVED by this helper) ===' 'Cyan'
    Write-ReportLine '  The helper makes no claim about the items below. Record them by hand in' 'Gray'
    Write-ReportLine ('  docs/human-acceptance-v{0}.md:' -f $script:AppVersion) 'Gray'
    $items = @(
        'Tray presence: the Rate Limits tray icon appears after launch (section 2)',
        'Open from tray: Open shows the dashboard (section 2)',
        'Close hides to tray: visually confirm the window hides and the tray icon remains (section 2)',
        'Reopen from tray: Open shows the dashboard again (section 2)',
        'Provider cards: every supported provider card loads and refreshes; no mock provider cards appear in the production build (section 3)',
        'Cached / Stale / Refresh failed wording understandable (section 3)',
        'Themes render: Graphite default on first launch, Glass, OLED; settings readable in each (section 4)',
        'Narrow width 360-400 px: no obvious clipping (section 4)',
        'Countdown: absolute reset time and relative countdown visible, advances ~every 30s, never negative (section 5)',
        'Prediction gating: no prediction block while history insufficient; confidence labeling (section 6)',
        'Clear history works; refresh keeps working after clearing (section 6)',
        ('Installer: displayed app version is {0}; installer runs and installs (section 8)' -f $script:AppVersion)
    )
    foreach ($item in $items) {
        Write-ReportLine ("  [NOT OBSERVED] {0}" -f $item) 'Gray'
    }
}

function Write-ReportFile {
    # Lazily default the report dir so that even an error-flush can write a
    # report before parameter resolution has run. Note: assignments here use
    # $script: scope explicitly so the bound $ReportDir parameter is untouched.
    if ([string]::IsNullOrEmpty($script:ReportDir)) { $script:ReportDir = (Get-Location).Path }
    $dir = $script:ReportDir
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $path = Join-Path $dir ("human-acceptance-v{0}-{1}.txt" -f $script:AppVersion, $stamp)

    $footer = @()
    $footer += ''
    $footer += '=== FINAL STATE / DISCLAIMER ==='
    if ([string]::IsNullOrEmpty($script:ProcessName)) {
        $footer += '  No process detection configured (installer-only run).'
    }
    else {
        $endSnap = Get-CountInfo -Snapshot (Get-AppSnapshot)
        if ($endSnap.RunningCount -gt 0) {
            $footer += ("  NOTE: {0} matching process(es) still running when the helper exited. The helper did not kill them." -f $endSnap.RunningCount)
            foreach ($line in (Format-SnapshotEvidence -Snapshot (Get-AppSnapshot))) { $footer += $line }
        }
        else {
            $footer += '  No matching processes were running when the helper exited.'
        }
    }
    $footer += '  This report records MECHANICAL evidence only. It does not by itself PASS or FAIL'
    $footer += '  Human Acceptance. Tray icon, UI, theme, countdown, and prediction/history'
    $footer += ('  correctness are judged solely by the human operator, per docs/human-acceptance-v{0}.md.' -f $script:AppVersion)
    $footer += ('  Run duration: {0:0}s.' -f ((Get-Date) - $script:RunStartedAt).TotalSeconds)

    $all = @($script:Report) + $footer
    $all | Set-Content -LiteralPath $path -Encoding UTF8

    Write-ReportLine '' 'White'
    Write-ReportLine ("  Report written to: {0}" -f $path) 'Cyan'
    foreach ($line in $footer) { Write-Host $line -ForegroundColor 'Gray' }

    # Git hygiene note (report location must not sneak into a commit)
    $ignored = $null
    $repoRoot = $null
    try { $repoRoot = Split-Path -Parent $PSScriptRoot } catch { }
    if ($repoRoot -and (Get-Command git -ErrorAction SilentlyContinue)) {
        try {
            $null = & git -C $repoRoot check-ignore -q -- $path 2>$null
            $ignored = ($LASTEXITCODE -eq 0)
        }
        catch { $ignored = $null }
    }
    if ($ignored -eq $false) {
        Write-ReportLine '  NOTE: the report directory is NOT git-ignored in this repository. Do not commit' 'Yellow'
        Write-ReportLine '  personal acceptance reports (or pass a -ReportDir outside the repo, e.g. %TEMP%).' 'Yellow'
    }
    return $path
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
$ErrorActionPreference = 'Stop'

try {
    if ($env:OS -ne 'Windows_NT') {
        Write-Host 'This helper is Windows-only.' -ForegroundColor Red
        exit 2
    }

    # --- parameter resolution ------------------------------------------------
    $appPhasesCanonical = @('Preflight', 'Launch', 'SingleInstance', 'CloseTray', 'Quit', 'Restart')
    $requestedPhases = @($Phases -split '[,;]' | ForEach-Object { $_.Trim() } | Where-Object { -not [string]::IsNullOrEmpty($_) })
    if ($PreflightOnly) { $requestedPhases = @('Preflight') }
    $unknownPhases = @($requestedPhases | Where-Object { $appPhasesCanonical -notcontains $_ })
    if ($unknownPhases.Count -gt 0) {
        Write-Host ("Unknown phase(s): {0}. Valid phases: {1}" -f ($unknownPhases -join ', '), ($appPhasesCanonical -join ', ')) -ForegroundColor Red
        exit 2
    }
    # canonical order, independent of how the operator listed them
    $appPhasesSelected = @($appPhasesCanonical | Where-Object { $requestedPhases -contains $_ })
    if (-not [string]::IsNullOrEmpty($ExePath)) {
        if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
            Write-Host ("Executable not found: {0}" -f $ExePath) -ForegroundColor Red
            exit 2
        }
        $script:ExePath = (Get-Item -LiteralPath $ExePath).FullName
    }
    else {
        $script:ExePath = $null
    }
    if (-not [string]::IsNullOrEmpty($InstallerPath)) {
        $script:InstallerPath = [System.IO.Path]::GetFullPath($InstallerPath)
    }
    else {
        $script:InstallerPath = $null
    }

    $appPhasesSelected = @($appPhasesCanonical | Where-Object { $requestedPhases -contains $_ })
    if (-not $script:ExePath) { $appPhasesSelected = @() }

    if (-not $script:ExePath -and -not $script:InstallerPath) {
        Write-Host 'Usage: pass -ExePath (and optionally -InstallerPath), or -InstallerPath alone for a read-only installer inspection.' -ForegroundColor Red
        exit 2
    }

    if ($script:ExePath) {
        $script:ProcessName = if ([string]::IsNullOrEmpty($ProcessName)) {
            [System.IO.Path]::GetFileNameWithoutExtension($script:ExePath)
        }
        else { $ProcessName }
    }
    else {
        $script:ProcessName = $ProcessName
    }

    if ([string]::IsNullOrEmpty($ReportDir)) {
        $repoRoot = $null
        try { $repoRoot = Split-Path -Parent $PSScriptRoot } catch { }
        if ([string]::IsNullOrEmpty($repoRoot)) { $repoRoot = (Get-Location).Path }
        $script:ReportDir = Join-Path $repoRoot 'artifacts'
    }
    else {
        $script:ReportDir = [System.IO.Path]::GetFullPath($ReportDir)
    }

    # --- header ---------------------------------------------------------------
    Write-ReportLine ('=' * 78) 'Cyan'
    Write-ReportLine ('{0} v{1} - HUMAN ACCEPTANCE HELPER (non-destructive)' -f $script:AppDisplayName, $script:AppVersion) 'Cyan'
    Write-ReportLine ('=' * 78) 'Cyan'
    Write-ReportLine ("  Run started : {0}" -f $script:RunStartedAt.ToString('yyyy-MM-dd HH:mm:ss zzz'))
    Write-ReportLine ("  Host        : {0} / {1} / PowerShell {2}" -f `
            $env:COMPUTERNAME, $env:USERNAME, $PSVersionTable.PSVersion.ToString())
    try {
        $os = (Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop).Caption
        Write-ReportLine ("  OS          : {0}" -f $os)
    }
    catch { Write-ReportLine '  OS          : (query unavailable)' 'Gray' }
    Write-ReportLine ("  Exe         : {0}" -f $(if ($script:ExePath) { $script:ExePath } else { '(not supplied - installer-only run)' }))
    if ($script:ExePath) {
        Write-ReportLine ("  Process name: {0}" -f $script:ProcessName)
        Write-ReportLine ("  Phases      : {0}" -f ($appPhasesSelected -join ', '))
        Write-ReportLine ("  StopExisting: {0}  (operator opt-in cleanup; exact image-path match only)" -f $script:StopExisting)
    }
    if ($script:InstallerPath) { Write-ReportLine ("  Installer   : {0}  (read-only inspection)" -f $script:InstallerPath) }
    Write-ReportLine ("  Report dir  : {0}" -f $script:ReportDir)
    Write-ReportLine '  Safety      : no credential/auth/provider file changes, no registry/autostart edits,' 'Gray'
    Write-ReportLine '                no installs/uninstalls, no app-data or localStorage changes, and no' 'Gray'
    Write-ReportLine '                process is ever killed except via explicit -StopExisting path matches.' 'Gray'

    # executable-present is always evaluated when an exe is supplied
    if ($script:ExePath) {
        Add-Check -Id 'executable-present' -Result 'PASS' -Detail $script:ExePath
    }

    # --- phases ---------------------------------------------------------------
    foreach ($phase in @('Preflight', 'Launch', 'SingleInstance', 'CloseTray', 'Quit', 'Restart')) {
        if ($appPhasesSelected -notcontains $phase) { continue }
        if ($script:Aborted) {
            Add-Check -Id $script:PhaseCheckId[$phase] -Result 'NOT OBSERVED' -Detail 'run aborted before this phase'
            if ($phase -eq 'Restart') {
                Add-Check -Id 'restart-theme-persisted' -Result 'NOT OBSERVED' -Detail 'run aborted before this phase'
                Add-Check -Id 'restart-history-persisted' -Result 'NOT OBSERVED' -Detail 'run aborted before this phase'
            }
            continue
        }
        switch ($phase) {
            'Preflight' { Invoke-PreflightPhase }
            'Launch' { Invoke-LaunchPhase }
            'SingleInstance' { Invoke-SingleInstancePhase }
            'CloseTray' { Invoke-CloseTrayPhase }
            'Quit' { Invoke-QuitPhase }
            'Restart' { Invoke-RestartPhase }
        }
    }

    # phases that were not selected -> NOT OBSERVED (never silently missing)
    if ($script:ExePath) {
        foreach ($phase in @('Preflight', 'Launch', 'SingleInstance', 'CloseTray', 'Quit', 'Restart')) {
            if ($appPhasesSelected -contains $phase) { continue }
            if (-not $script:Checks.Contains($script:PhaseCheckId[$phase])) {
                Add-Check -Id $script:PhaseCheckId[$phase] -Result 'NOT OBSERVED' -Detail 'phase not selected in this run'
            }
        }
        if ($appPhasesSelected -notcontains 'Restart') {
            if (-not $script:Checks.Contains('restart-theme-persisted')) {
                Add-Check -Id 'restart-theme-persisted' -Result 'NOT OBSERVED' -Detail 'restart phase not selected in this run'
            }
            if (-not $script:Checks.Contains('restart-history-persisted')) {
                Add-Check -Id 'restart-history-persisted' -Result 'NOT OBSERVED' -Detail 'restart phase not selected in this run'
            }
        }
    }

    if ($script:InstallerPath) { Invoke-InstallerInspection }

    # --- summary + report -----------------------------------------------------
    Write-ChecksSummary
    Write-ManualChecklist
    $null = Write-ReportFile

    exit $(if ($script:Aborted) { 2 } else { 0 })
}
catch {
    Write-Host ('UNEXPECTED ERROR: {0}' -f $_.Exception.Message) -ForegroundColor Red
    Write-Host $_.ScriptStackTrace -ForegroundColor DarkRed
    try {
        $script:Report.Add('UNEXPECTED ERROR: ' + $_.Exception.Message) | Out-Null
        $null = Write-ReportFile
    }
    catch { }
    exit 1
}
