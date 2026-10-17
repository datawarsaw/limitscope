# LimitScope v0.8.12 — Release Verification Record

Recorded on the v0.8.12 candidate tree that contains this file. The release SHA is that commit.

## Live provider diagnostic

Procedure: `powershell -NoProfile -File scripts/provider-quota-diagnostic.ps1`. The procedure runs the production-provider live probe, including Qwen Token Plan, and its credential-shape output scan. Account hints, raw percentages, and raw diagnostic logs are intentionally excluded.

| Provider | Checked at (UTC) | Result | Evidence |
| --- | --- | --- | --- |
| OpenAI / Codex | 2026-10-17T13:38:52Z | PASS | health `live`; weekly window returned |
| Z.ai | 2026-10-17T13:38:52Z | FAIL | health `error`; `unexpected_response` |
| OpenCode Go | 2026-10-17T13:38:52Z | FAIL | health `error`; `not_entitled` |
| Google Antigravity | 2026-10-17T13:38:52Z | PASS | health `live`, fresh; Gemini and Gemini Weekly windows returned |
| Grok (xAI) | 2026-10-17T13:38:52Z | PASS | health `live`; weekly credits returned |
| Qwen Token Plan | 2026-10-17T13:38:52Z | PASS | health `live`; monthly window returned |

Secret scan: PASS. Z.ai and OpenCode Go remain FAIL and are not waived or reclassified. OpenDesign is not a production provider in this release: no verified billing command was available, so it has no diagnostic row and is not reported as live.

