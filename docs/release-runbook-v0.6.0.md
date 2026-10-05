# LimitScope v0.6.0 — Release Runbook and Delivery Record

Companion: [v0.6.0-release-candidate.md](v0.6.0-release-candidate.md) (Human Acceptance: PASS, recorded 2026-09-30).

## Release Identity and Provenance

| Item | Value |
| --- | --- |
| Version | `0.6.0` |
| Product name | `LimitScope` |
| Internal identifier | `com.ratelimits.desktop` (unchanged) |
| Authoritative tested product SHA | `a9585c1763a0c442bc0ac861ddd8722b1bf30c26` |
| Accepted RC head | `384f20e35131aa82d436e1931f64dc684fa97914` (docs/state Human Acceptance record) |
| Merge commit (into main) | `7b6decf6847445e88105ce2b528f7d93087c9e81` |
| Release SHA (main head) | `69c7f0ad13b04f6cbf7648edb53a30585b0a704a` |
| Annotated tag | `v0.6.0` |
| Tag object SHA | `d9846a1d54374744e4243c24af15388d5552e243` |
| Tag resolved commit | `69c7f0ad13b04f6cbf7648edb53a30585b0a704a` |
| Source repository | `datawarsaw/rate-limits` |
| Source GitHub Release | https://github.com/datawarsaw/rate-limits/releases/tag/v0.6.0 |
| Public updater repository | `datawarsaw/limitscope-releases` |
| Public updater Release | https://github.com/datawarsaw/limitscope-releases/releases/tag/v0.6.0 |
| Public feed URL | https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json |

## Authoritative Production Artifact

- **Production installer:** `LimitScope_0.6.0_x64-setup.exe`
- **Installer size:** 2,590,027 bytes
- **Production SHA-256:** `fc32f63b96683a9f56817691c36637e0b4da977e1438aebdfff1ce0d4367cf1d`
- **RC local installer SHA-256:** `bdebe4eef9f208cd6d376fa96c5c5fbba3788d89ee08ce5e7c9c756d28bb0eed`
- **Provenance distinction:** Official CI rebuild on `windows-latest` from the accepted release source.
- **FileVersion / ProductVersion / ProductName:** `0.6.0` / `0.6.0` / `LimitScope`
- **Signature:** Valid minisign signature covering version `0.6.0` against embedded public key.

## Official Release Workflow Execution

- **Workflow:** `.github/workflows/release.yml`
- **Run ID:** `36733251523`
- **Trigger:** `workflow_dispatch` (`version=0.6.0`, `notes=LimitScope v0.6.0`)
- **Status / Conclusion:** `completed` / `success`
- **Head commit:** `69c7f0ad13b04f6cbf7648edb53a30585b0a704a`

## Verification Checklist

| Gate | Target | Result | Evidence |
| --- | --- | --- | --- |
| Tree identity | RC product tree == Release tree | PASS | Zero diff under `src/`, `src-tauri/src/`, manifests |
| Annotated tag | `v0.6.0` points to RELEASE_SHA | PASS | `git rev-parse v0.6.0^{}` == `69c7f0a` |
| Minisign signature | Bound to 0.6.0 & embedded pubkey | PASS | Signature verified against installer |
| Unauthenticated download | Public companion assets reachable | PASS | `latest.json`, `.exe`, `.sig` return HTTP 200/302 |
| Atomic publication | `latest.json` published last | PASS | Companion assets available prior to feed update |
| Updater E2E | Controlled production updater | PARTIAL | Passive NSIS `/P /R` verified; app relaunches on 0.6.0 |
| Native acceptance smoke | Core lifecycle & single instance | PASS | Launch, single instance, tray icon, menu commands, settings preservation |
| Provider diagnostic | 5 production providers live | PASS | `openai-codex`, `zai`, `opencode-go`, `antigravity`, `grok` all reporting live |
| Secret audit | Clean repository & build artifacts | PASS | Zero secrets detected by `npm run scan:secrets` |
| v0.7 leakage | Zero v0.7 features in v0.6.0 | NONE | No execution provenance/runs/receipts/credits/telemetry |

## RC Archive Status

- Branch `release/v0.6.0-rc` and worktree `C:\AI\Token_Monitor_v060_rc` are marked as archive/removal candidates.
- Release provenance retained on `main` and tag `v0.6.0`.
