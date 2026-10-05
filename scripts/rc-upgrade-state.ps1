# LimitScope v0.5.0 RC - upgrade smoke state probe.
# Read-only: reports ARP entries, Run-key autostart, install dirs, app-data dirs.
param(
  [switch]$Json
)

$ErrorActionPreference = "SilentlyContinue"

$arp = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
  ForEach-Object { Get-ItemProperty $_.PSPath } |
  Where-Object { $_.DisplayName -match '^(Rate Limits|LimitScope)$' } |
  Select-Object DisplayName, DisplayVersion, InstallLocation, UninstallString

$arpHklm = Get-ChildItem 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
  ForEach-Object { Get-ItemProperty $_.PSPath } |
  Where-Object { $_.DisplayName -match '^(Rate Limits|LimitScope)$' } |
  Select-Object DisplayName, DisplayVersion, InstallLocation, UninstallString

$run = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).PSObject.Properties |
  Where-Object { $_.Name -match 'Rate Limits|LimitScope' } |
  Select-Object @{n='Name';e={$_.Name}}, @{n='Value';e={$_.Value}}

$dirs = [ordered]@{
  localappdataRateLimits        = Test-Path "$env:LOCALAPPDATA\Rate Limits"
  localappdataLimitScope        = Test-Path "$env:LOCALAPPDATA\LimitScope"
  identifierComDataDir          = Test-Path "$env:LOCALAPPDATA\com.ratelimits.desktop"
  identifierWebView2            = Test-Path "$env:LOCALAPPDATA\com.ratelimits.desktop\EBWebView"
  historyFile                   = (Test-Path "$env:LOCALAPPDATA\com.ratelimits.desktop") -and ((Get-ChildItem "$env:LOCALAPPDATA\com.ratelimits.desktop" -Recurse -File -ErrorAction SilentlyContinue | Where-Object Name -match 'history|quota').Count -gt 0)
  notificationStateFile         = (Get-ChildItem "$env:LOCALAPPDATA\com.ratelimits.desktop" -Recurse -File -ErrorAction SilentlyContinue | Where-Object Name -match 'notification').Count -gt 0
}

$exe = Get-Process -Name 'rate-limits','LimitScope' -ErrorAction SilentlyContinue | Select-Object Name, Id, Path

$result = [ordered]@{
  arpHkcu       = @($arp)
  arpHklm       = @($arpHklm)
  runEntries    = @($run)
  dirs          = $dirs
  processes     = @($exe)
}

if ($Json) {
  $result | ConvertTo-Json -Depth 4
} else {
  "=== ARP (HKCU) ==="; $arp | Format-List
  "=== ARP (HKLM) ==="; $arpHklm | Format-List
  "=== Run entries ==="; $run | Format-List
  "=== Dirs ==="; $dirs.GetEnumerator() | ForEach-Object { "{0} = {1}" -f $_.Key, $_.Value }
  "=== Processes ==="; $exe | Format-Table -AutoSize
}
