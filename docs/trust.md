# Trust, but verify

LimitScope sits in your system tray, reads credentials your provider tools
already store on disk, and queries provider usage endpoints. Those are the
right things to be skeptical about, so this page states what the app claims,
how you can check each claim yourself on your own machine, and where each
check stops short of proof.

These notes describe the current source checkout, which includes work made
after the public **v0.8.6** release. In particular, the ZCode reset-status
request below is not present in the v0.8.6 installer. The verification
commands are read-only PowerShell, need no administrator rights, and do not
print credential material. Source references point to the code in this
repository; runtime checks let you inspect the installed app on your machine.

## The claims

1. **No telemetry.** Nothing is sent to analytics, advertising, or
   crash-reporting services. The only outbound traffic is provider quota
   requests and the update check.
2. **Provider traffic goes only to the providers' own usage endpoints** —
   never to a LimitScope server (none exists).
3. **Credentials stay out of the interface.** Keys and tokens are read and
   used inside the Rust process; the web view only ever receives normalized
   quota windows and masked account labels.
4. **Foreign credential stores are read-only.** LimitScope reads existing
   logins; it never writes, rotates, or refreshes them.
5. **Diagnostics are inclusion-based and redacted.** An exported support
   bundle has a fixed field list; credentials and raw provider responses
   cannot enter it by construction.
6. **Local state is bounded and app-owned.** A few JSON stores and browser
   profile preferences, each clearable from Settings.
7. **Updates are signed and verified.** The installer signature is checked
   against a key embedded in the app before an update is ever executed.

## What data leaves your machine

Outbound traffic consists of provider quota and supplementary status
requests, plus the update check:

| Class | Destination (current source) |
| --- | --- |
| OpenAI / Codex | `GET https://chatgpt.com/backend-api/wham/usage` |
| Z.ai | `GET https://api.z.ai/api/monitor/usage/quota/limit` |
| OpenCode Go | `GET https://opencode.ai/zen/go/v1/usage` |
| Google Antigravity | Google's OAuth token endpoint and `GET https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary` (live-first); falls back to the local cache the OpenCode Antigravity plugin wrote, with no network call |
| xAI Grok | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` |
| ZCode reset cards | `GET https://zcode.z.ai/api/v1/coding-plan/reset/status` (read-only supplementary observation using credentials from ZCode's local store) |
| Updater | `https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json` (feed check); the installer itself is downloaded only when you click **Update now** |

The web view cannot open network connections of its own: its content
security policy restricts it to the app bundle and the local IPC bridge, and
its capability set contains no filesystem, shell, or raw-HTTP permissions.
All HTTP lives in the Rust process.

**Verify — watch the process's connections** while a refresh runs (or right
after launch, when the silent update check fires):

```powershell
$p = Get-Process LimitScope | Select-Object -First 1
Get-NetTCPConnection -OwningProcess $p.Id -State Established |
    Sort-Object RemoteAddress -Unique |
    Select-Object RemoteAddress, RemotePort
```

You will see numeric addresses, not host names — provider and GitHub hosts
sit behind CDNs, so the IPs vary. Resource Monitor's Network tab shows the
same data with resolution. Absence of a connection during one check shows
only that no connection existed at that moment.

**Limitations.** These are unofficial or semi-private provider surfaces that
can change without notice; the app's contract for that is a structured error
card, never a crash. The check observes traffic during one run — it cannot
prove what every future run does, and it covers the `LimitScope` process
only: WebView2 and Windows are platform components with network behavior of
their own, outside this application's source.

## What is stored locally

LimitScope writes only its own bounded stores:

- `%APPDATA%\com.ratelimits.desktop\quota-history-v1.json` — quota
  observations (24 hours of detail, 7 days total, capped samples per window)
- `%APPDATA%\com.ratelimits.desktop\provider-last-good-v1.json` — last-known
  quota cache for the next start (dropped after 7 days)
- `%APPDATA%\com.ratelimits.desktop\quota-notifications-v1.json` — bounded
  notification bookkeeping
- WebView2 profile storage under the same app identifier — preferences
  (`rate-limits.settings.v1`, `rate-limits.floating-quota.v1`) and bounded
  execution-run records (`limitscope.execution-runs.v1`)
- one `HKCU\...\CurrentVersion\Run` value, only if you enable launch at
  startup

Everything except the registry entry has a clear or reset control under
**Settings → Local data** and **Preferences**, and each control removes only
its own category.

**Verify — list the app's files:**

```powershell
Get-ChildItem "$env:APPDATA\com.ratelimits.desktop" -File |
    Select-Object Name, Length, LastWriteTime
```

**Limitations.** The app does write these files — "app-owned" does not mean
"nothing is written locally". The WebView2 profile directory is managed by
the WebView2 runtime; its internals are platform territory.

## Credentials and foreign stores

LimitScope reads existing credentials from the tools it supports, per
provider: the Codex CLI login (`~/.codex/auth.json`), the Z.ai coding-plan
key held by ZCode's own stores, the OpenCode Go key held by OpenCodex or
OpenCode, the quota cache the OpenCode Antigravity plugin maintains, and the
xAI credential held by the Grok CLI or the OpenCode stores. The contract:

- reads are read-only; no foreign store is ever written,
- credentials are never rotated — Codex and Grok OAuth tokens are
  deliberately never refreshed by LimitScope, because doing so could
  invalidate the owning tool's stored session; Antigravity exchanges the
  stored Google refresh token for short-lived access tokens held in memory
  only,
- decryption of ZCode's encrypted entries happens inside the Rust process,
- nothing secret is ever written to LimitScope's own stores either.

**Verify — hash a foreign credential file across a refresh cycle** (Codex
shown; the same works for the other stores):

```powershell
Get-FileHash "$env:USERPROFILE\.codex\auth.json" -Algorithm SHA256
# ...run a refresh cycle in LimitScope...
Get-FileHash "$env:USERPROFILE\.codex\auth.json" -Algorithm SHA256
```

Identical hashes across the cycle mean the file was not modified during that
window.

**Limitations.** The owning tools write these files during their own normal
operation (logins, their own token refreshes) — compare across a LimitScope
refresh only. Equal hashes during one window are evidence, not a permanent
guarantee.

## Diagnostics and redaction

**Settings → Export diagnostics** writes one JSON bundle with a fixed,
deliberate field list: build identity, runtime cycle timing, per-provider
normalized health with a masked account label, bounded history metadata,
notification counters, and settings. Excluded by construction: access and
refresh tokens, API keys, JWTs, cookies, Authorization headers, raw provider
response bodies, raw exception dumps, credential stores, full history,
machine username, home directory, absolute paths, and hardware inventory.
Provider error text is replaced with fixed local messages, every remaining
free-form string passes a sanitizer, and the bundle is bounded at 2 MiB and
written atomically through the system save dialog.

**Verify — export a bundle and count secret-shaped strings without printing
them:**

```powershell
Set-Location "<the folder where you saved the bundle>"
(Select-String -Path "runtime-diagnostics-*.json" -SimpleMatch -Pattern "sk-","Bearer ","eyJ").Count
```

The expected count is `0`.

**Limitations.** Redaction is engineering — a fixed serialization boundary
plus sanitizers — not a mathematical proof of zero leakage, and one clean
bundle is a spot check of one moment. A count above zero can also be a false
positive (the bundle legitimately contains masked labels); if you see one,
inspect the bundle carefully before sharing it.

## Updates and release integrity

The updater checks an unauthenticated HTTPS feed on a public GitHub
repository (`datawarsaw/limitscope-releases`) — the same place release
assets live. Installers are minisign-signed at build time; the downloaded
update's signature is verified against the public key embedded in the app
before the installer is executed, an invalid signature means the download is
rejected and deleted, and only a strictly newer version is ever offered.
Nothing installs without your explicit **Update now**; the silent startup
check only surfaces an indicator, and a failed check never interrupts quota
monitoring.

**Verify — inspect the feed and the release assets:**

```powershell
Invoke-RestMethod "https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json"
```

Compare the reported version with **Settings → Updates**, and check on the
releases page that every installer ships with its `.sig` signature file. The
minisign public key is embedded in the application (`plugins.updater.pubkey`
in the source-of-record configuration).

**Limitations.** This is minisign integrity, not Windows Authenticode: the
installer is not certificate-signed, so SmartScreen will warn about an
unknown publisher. Signature verification proves an update was authorized by
the current signing key — not that the code is bug-free. Releases are cut
from the source repository by a pinned workflow (version agreement across
all version surfaces, a SHA-256 and size manifest, and the feed published
last). Public source makes the code and workflow inspectable; it does not by
itself provide bit-for-bit reproducible builds. The trust root remains the
embedded signing key.

## What this cannot prove

Honest limits of every check on this page:

- **Claims describe the current code, not every future build.** Source- and
  test-anchored claims are evidence of what the 0.8.4 lineage does today.
  There is no public reproducible-build path, so byte-identity between a
  release and its source of record cannot be independently recomputed by
  you.
- **Network observation is a snapshot.** Watching connections during one run
  shows what happened during that run, on that machine, at the IP level —
  not a guarantee about every run, and endpoints behind shared CDNs cannot
  always be distinguished by address.
- **Provider endpoints can change.** The quota surfaces the app queries are
  unofficial or semi-private and may change shape or disappear without
  notice; the app treats that as an error state, not a security event.
- **Platform components are out of scope.** Windows, WebView2, and the
  network stack have behavior outside this application's source and outside
  these claims.
- **A verified signature is not a safety guarantee.** It authenticates who
  authorized an update, not that the update is free of bugs.
- **Redaction is not proof of zero leakage.** Diagnostics reduce leak risk by
  construction; they cannot prove the absence of every future bug.
- **No independent audit is claimed.** No third-party audit or penetration
  test has been performed, and this page is not a substitute for one.
