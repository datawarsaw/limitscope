# LimitScope v0.8.11 — Release Notes

This release brings the accepted updates on canonical `main` through `3c749e138bc271009d9fe5da17662f9481bd3b4a`.

## Highlights

- **Gemini Weekly in the floating quota view.** The designated long-term usage window is now selected and presented consistently alongside short-term quota, with matching remaining/used semantics and focused coverage for the selection rules.
- **Grok Bot usage and manual refresh.** Observe Grok Bot (X Premium+) weekly usage as a supplemental pool. The floating view supports an explicit refresh action, with isolated handling for manual fetch outcomes.
- **Release and CI reliability.** CI avoids rebuilding NSIS when packaging inputs have not changed, while retaining the full release build gate when they do.

The release preserves the existing provider trust boundaries and updater signing configuration. The updater feed will carry these notes after the signed release workflow verifies the exact tagged source SHA.