#Requires -Version 5.1
<#
.SYNOPSIS
  Repeatable performance snapshot for LimitScope (v0.6 performance budget).
.DESCRIPTION
  Collects binary and bundle sizes, cold-start timing, working-set samples,
  idle CPU, and WebView2 child processes. Paste the SUMMARY TABLE into
  docs/research/performance-budget-v0.6.md section 7 for the UNVERIFIED rows.
  Read-only except for launching and stopping the app under test.
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/perf-snapshot.ps1
#>
param(
  [string]$E = (Join-Path $PSScriptRoot "../src-tauri/target/release/rate-limits.exe"),
  [int]$ColdStartRuns = 3,
  [int]$IdleSeconds = 120,
  [int]$SampleEverySeconds = 10,
  [switch]$KeepRunning
)
$ErrorActionPreference = "Stop"
function Show-Row {
  param([string]$Name, [string]$Value, [string]$Budget, [string]$Verdict)
  [pscustomobject]@{ Metric = $Name; Observed = $Value; Budget = $Budget; Verdict = $Verdict }
}
$rows = @()
# 1. Artifacts on disk
$exe = Get-Item -LiteralPath $E
$js = Get-ChildItem (Join-Path $PSScriptRoot "../dist/assets/*.js") | Sort-Object Length -Descending | Select-Object -First 1
$css = Get-ChildItem (Join-Path $PSScriptRoot "../dist/assets/*.css") | Sort-Object Length -Descending | Select-Object -First 1
$exeMiB = [math]::Round($exe.Length / 1MB, 2)
$rows += Show-Row "binary on disk" ($exeMiB.ToString() + " MiB") "<= 8 MiB (sanity)" $(if ($exeMiB -le 8) { "PASS" } else { "FAIL" })$rows += Show-Row "frontend JS bundle" ([math]::Round($js.Length / 1KB, 1).ToString() + " KiB") "<= 500 KiB (sanity)" $(if ($js.Length -le 512000) { "PASS" } else { "FAIL" })
$rows += Show-Row "frontend CSS bundle" ([math]::Round($css.Length / 1KB, 1).ToString() + " KiB") "<= 100 KiB (sanity)" $(if ($css.Length -le 102400) { "PASS" } else { "FAIL" })
# 2. Cold start: launch hidden (tray-only), time until CPU accumulates
$coldStarts = @()
for ($i = 1; $i -le $ColdStartRuns; $i++) {
  Get-Process -Name "LimitScope" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $exe.FullName -ArgumentList "--hidden" -PassThru
  $readyMs = $null
  for ($t = 0; $t -lt 60; $t++) {
    Start-Sleep -Milliseconds 500
    try { $p = Get-Process -Id $p.Id -ErrorAction Stop } catch { break }
    if ($p.TotalProcessorTime.TotalMilliseconds -gt 300) { $readyMs = $sw.Elapsed.TotalMilliseconds; break }
  }
  $sw.Stop()
  if ($null -ne $readyMs) { $coldStarts += $readyMs }
  Write-Host ("cold start run " + $i + ": tray-ready proxy " + [math]::Round($readyMs / 1000, 1) + " s")
  Get-Process -Name "LimitScope" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
}
if ($coldStarts.Count -gt 0) {
  $sx = $coldStarts | Sort-Object
  $med = $sx[[math]::Floor($sx.Count / 2)]
  $rows += Show-Row "cold start to tray (proxy)" ([math]::Round($med / 1000, 1).ToString() + " s") "<= 4 s" $(if ($med -le 4000) { "PASS" } else { "FAIL" })
} else {
  $rows += Show-Row "cold start to tray (proxy)" "no sample" "<= 4 s" "UNVERIFIED"
}# 3. Idle soak: leave in tray, sample CPU and working set
Get-Process -Name "LimitScope" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
$app = Start-Process -FilePath $exe.FullName -ArgumentList "--hidden" -PassThru
Start-Sleep -Seconds 15
$wsSamples = @()
$cpuStart = (Get-Process -Id $app.Id).TotalProcessorTime
$wallStart = Get-Date
$n = [math]::Max(1, [math]::Floor($IdleSeconds / $SampleEverySeconds))
for ($s = 0; $s -lt $n; $s++) {
  Start-Sleep -Seconds $SampleEverySeconds
  $pa = @(Get-Process -Name "LimitScope" -ErrorAction SilentlyContinue)
  $wv = @(Get-Process -Name "msedgewebview2" -ErrorAction SilentlyContinue)
  $totalWs = 0
  foreach ($q in ($pa + $wv)) { try { $totalWs += $q.WorkingSet64 } catch { } }
  $wsSamples += $totalWs
}
$cpuEnd = (Get-Process -Id $app.Id -ErrorAction SilentlyContinue).TotalProcessorTime
$wallSecs = ((Get-Date) - $wallStart).TotalSeconds
$cpuPct = "n/a"
if ($null -ne $cpuEnd) { $cpuPct = [math]::Round((($cpuEnd - $cpuStart).TotalSeconds / $wallSecs / [Environment]::ProcessorCount) * 100, 2) }
$wsSorted = $wsSamples | Sort-Object
$wsMedMiB = [math]::Round($wsSorted[[math]::Floor($wsSorted.Count / 2)] / 1MB, 1)
$wsMaxMiB = [math]::Round($wsSorted[-1] / 1MB, 1)
$rows += Show-Row "idle CPU avg (tray-only)" ($cpuPct.ToString() + " %") "< 1 %" $(if ($cpuPct -eq "n/a") { "UNVERIFIED" } elseif ([double]$cpuPct -lt 1) { "PASS" } else { "FAIL" })
$rows += Show-Row "working set median (app+webview)" ($wsMedMiB.ToString() + " MiB") "<= 350 MiB" $(if ($wsMedMiB -le 350) { "PASS" } else { "FAIL" })
$rows += Show-Row "working set max (app+webview)" ($wsMaxMiB.ToString() + " MiB") "<= 350 MiB" $(if ($wsMaxMiB -le 350) { "PASS" } else { "FAIL" })
$wvCount = @(Get-Process -Name "msedgewebview2" -ErrorAction SilentlyContinue).Count
$rows += Show-Row "webview2 processes (tray-only)" $wvCount "record only" "INFO"
if (-not $KeepRunning) {
  Get-Process -Name "LimitScope" -ErrorAction SilentlyContinue | Stop-Process -Force
  Write-Host "app under test stopped."
} else {
  Write-Host "app under test left running (-KeepRunning). Quit via the tray menu."
}# 4. Summary table
Write-Host ""
Write-Host "================ SUMMARY TABLE (paste into performance-budget-v0.6.md section 7) ================"
$rows | Format-Table -AutoSize | Out-String | Write-Host
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os = Get-ComputerInfo -Property OsName, OsVersion
Write-Host ("machine: " + $cpu.Name.Trim() + " / " + [math]::Round($cs.TotalPhysicalMemory / 1GB, 0) + " GiB RAM / " + $os.OsName + " " + $os.OsVersion)
Write-Host ("date: " + (Get-Date -Format "yyyy-MM-dd HH:mm"))