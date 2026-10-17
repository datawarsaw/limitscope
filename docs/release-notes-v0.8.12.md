# LimitScope v0.8.12 — Release Notes

This release adds a read-only Qwen Cloud Token Plan provider for the international Singapore console.

## Highlights

- **Qwen Token Plan.** Monthly quota comes from the official Bailian CLI (`bl usage token-plan --console-site international --console-region ap-southeast-1`). `per1MonthPercentage` stays a 0–1 fraction of used quota, and `per1MonthResetTime` stays the epoch-millisecond reset. A missing CLI or console login is unavailable, never zero usage.
- **OpenDesign is not included.** Vela CLI 0.1.3 has no `billing preflight` command, and the CLI cannot complete a TLS handshake to the billing host. No usage provider was invented for it.

The release preserves the existing provider trust boundaries and updater signing configuration. The updater feed will carry these notes after the signed release workflow verifies the exact tagged source SHA.

