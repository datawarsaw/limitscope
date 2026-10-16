# LimitScope v0.8.11 — Release Verification Record

Canonical product baseline: `3c749e138bc271009d9fe5da17662f9481bd3b4a`.

## Live provider diagnostic

Procedure: `powershell -NoProfile -File scripts/provider-quota-diagnostic.ps1`. The procedure performs the five-provider live probe and its credential-shape output scan. Account hints and raw diagnostic logs are intentionally excluded.

| Provider | Checked at (UTC) | Result | Evidence |
| --- | --- | --- | --- |
| OpenAI / Codex | 2026-10-16T21:22:05Z | PASS | health `live`; weekly window returned |
| Z.ai | 2026-10-16T21:22:05Z | FAIL | health `error`; `unexpected_response` |
| OpenCode Go | 2026-10-16T21:22:05Z | FAIL | health `error`; `not_entitled` |
| Google Antigravity | 2026-10-16T21:22:05Z | PASS | health `live`, fresh; Gemini and Gemini Weekly windows returned |
| Grok (xAI) | 2026-10-16T21:22:05Z | PASS | health `live`; weekly credits returned |

Secret scan: PASS. The provider diagnostic completed successfully; the two provider health results above remain FAIL and are not waived or reclassified.

## Follow-up live provider diagnostic

Repeated the approved diagnostic to determine whether the two errors were transient. Results remained the same; each observation is retained.

| Provider | Checked at (UTC) | Result | Evidence |
| --- | --- | --- | --- |
| OpenAI / Codex | 2026-10-16T21:23:19Z | PASS | health `live` |
| Z.ai | 2026-10-16T21:23:19Z | FAIL | health `error`; `unexpected_response` |
| OpenCode Go | 2026-10-16T21:23:19Z | FAIL | health `error`; `not_entitled` |
| Google Antigravity | 2026-10-16T21:23:19Z | PASS | health `live`, fresh; Gemini Weekly returned |
| Grok (xAI) | 2026-10-16T21:23:19Z | PASS | health `live`; weekly credits returned |

Secret scan: PASS.
