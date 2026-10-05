# Security Policy

## Supported releases

Please report vulnerabilities affecting the latest stable LimitScope release (`v0.8.6`). Older releases are not guaranteed to receive security fixes; update to the latest stable release before reporting when practical.

## Reporting a vulnerability

Do not open a public issue for a vulnerability, credential exposure, or exploit details. Use GitHub's **private vulnerability reporting / security advisory** feature for this repository if it is enabled. If private reporting is not enabled, contact the repository owner through a verified private GitHub channel and ask for a secure reporting route. No security email address is currently published by this project.

Include the affected app version and Windows version, the relevant component, steps to reproduce, expected and observed behavior, and any safe logs or screenshots. Remove credentials, tokens, account identifiers, and personal data before sharing. Please allow the maintainer time to investigate and prepare a fix before public disclosure.

## In-scope areas

- Local credential discovery, handling, and redaction.
- Updater feed validation, signing, and installer verification.
- Provider request construction, redirects, and response handling.
- Local persistence and diagnostic/export data.
- Tauri commands, webview permissions, and desktop runtime security.

Reports about provider-side vulnerabilities should be sent to the relevant provider unless they demonstrate a LimitScope-specific security impact.
