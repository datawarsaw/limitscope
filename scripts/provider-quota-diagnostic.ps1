# Provider quota diagnostic — unified, redacted, read-only (all five providers).
#
# One invocation runs the sanctioned Rust probe (the ignored
# `five_provider_diagnostic_report` test in src-tauri) and prints one
# redacted, normalized row per production provider — Codex, Z.ai,
# OpenCode Go, Antigravity, Grok:
#
#   health (canonical: live/stale/unknown/cooldown/error/unavailable)
#   checkedAt, freshness / source stamp where meaningful
#   credential source label + masked account hint
#   per-window: label, used percent, reset timestamp
#
# A failed provider still produces a row: canonical health category, the
# normalized reason code (never the raw error message), and the masked
# attempted-account hint when the credential could be resolved.
#
# Secrets policy: credentials are read only in-process by the Rust backends;
# this script never touches credential stores itself. Raw upstream payloads
# are never printed (there is no -Raw mode anymore — the per-provider
# `live_fetch_returns_windows` tests and this probe replace it). The captured
# output is scanned for credential-shaped material (Bearer/Authorization,
# sk- keys, JWT shapes, token fields, PEM blocks) and the run fails loudly if
# any appears. Do not paste the output into tickets: it carries masked
# account hints, which are user-specific.
#
# Usage:  powershell -NoProfile -File scripts/provider-quota-diagnostic.ps1

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$outLog = Join-Path $env:TEMP "limitscope-provider-diagnostic-$stamp.log"
$errLog = Join-Path $env:TEMP "limitscope-provider-diagnostic-$stamp.err.log"

Write-Host "== LimitScope provider quota diagnostic (redacted, read-only) =="
Write-Host "Running the five-provider probe via cargo test (ignored live probe)..."
Write-Host ""

Push-Location (Join-Path $repoRoot "src-tauri")
try {
    # Build progress and warnings go to a temp stderr log so they can never
    # pollute the scanned stdout; they are shown only if the run fails.
    # ("Continue" here: PowerShell 5.1 turns native stderr lines — cargo's
    # progress output — into error records that would otherwise abort.)
    $ErrorActionPreference = "Continue"
    & cargo test --locked --bin rate-limits five_provider_diagnostic_report -- --ignored --nocapture --test-threads=1 2> $errLog | Tee-Object -FilePath $outLog
    $cargoExit = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($cargoExit -ne 0) {
        Write-Host "cargo test failed; build/test errors:"
        Get-Content $errLog -Tail 40 | ForEach-Object { Write-Host $_ }
        throw "the diagnostic probe failed"
    }
} finally {
    Pop-Location
}
Write-Host ""

# The probe's output is redacted by construction; scan the captured stdout
# for credential-shaped material anyway (belt and suspenders, same patterns
# as the Rust-side scan in src-tauri/src/diagnostic_report.rs).
$output = Get-Content $outLog -Raw
$patterns = [ordered]@{
    "Bearer token"         = "(?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}"
    "Authorization header" = "(?i)authorization\s*:"
    "sk- API key"          = "sk-[A-Za-z0-9_-]{8,}"
    "JWT-shaped token"     = "eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"
    "refresh token field"  = "(?i)refresh[_-]?token"
    "access token field"   = "(?i)access[_-]?token"
    # Any PEM block; the prefix alone is enough for an output scan.
    "PEM private key"      = "-----BEGIN"
}
$hits = @()
foreach ($entry in $patterns.GetEnumerator()) {
    if ($output -match $entry.Value) { $hits += $entry.Key }
}
if ($hits.Count -gt 0) {
    Write-Host "secret scan: FAIL — credential-shaped material found: $($hits -join ', ')"
    Write-Host "output saved to $outLog — do NOT commit or paste it"
    exit 1
}

Write-Host "secret scan: PASS (no credential-shaped material in the diagnostic output)"
Write-Host "redacted output saved to $outLog"
