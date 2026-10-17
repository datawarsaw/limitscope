# LimitScope v0.8.13 — Release Notes

This release adds a supplementary, read-only OpenDesign card. It reads the Go Design Plan's 5-hour and 7-day remaining shares from the signed-in OpenDesign page in Chrome through Windows UI Automation.

## Highlights

- **OpenDesign usage, on demand.** The card shows the plan ("OpenDesign Go"), the subscription period line the page displays, both allowance rows in page order, and each row's own reset countdown. Reading happens only when you press Refresh OpenDesign usage; there is no timer, scheduler, or background browser automation.
- **Remaining share, stated as the page states it.** The page reports the share left, so the card shows remaining percentages and never derives a "used" figure. The reset line is kept verbatim; an absolute reset timestamp is not invented from a relative countdown.
- **The pay-as-you-go wallet stays out.** Design Plan allowances and the separate wallet balance are never combined or substituted for one another.
- **Unavailable, stale, and error states are explicit.** A failed read keeps the last good pair with its original stamp and shows it as last known. No partial or single-window answer is accepted as a reading.
- **No credentials are touched.** The reader takes values from Chrome's accessibility tree only. It does not read cookies, tokens, storage, or headers, does not use the browser debug port, does not call the internal billing endpoint, and does not focus, reload, or type into Chrome.

## Operational condition

OpenDesign is not a runtime provider and does not appear in the provider rail or the automatic provider scan. A successful read requires Chrome to be running with an authenticated OpenDesign Plan & balance page open. When those conditions are absent, the card reports that clearly; it never shows a placeholder reading.

The release preserves the existing provider trust boundaries and updater signing configuration. The updater feed will carry these notes after the signed release workflow verifies the exact tagged source SHA.
