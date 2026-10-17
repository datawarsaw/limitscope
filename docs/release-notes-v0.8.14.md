# LimitScope v0.8.14

OpenDesign is now a standard provider in the dashboard, floating bar and quota
details, with the same visibility and ordering controls as other providers.

- Both independent allowances remain visible: 5-hour and 7-day remaining quota.
  The floating bar features the 7-day allowance.
- Source reset countdowns are labelled “At observation” and remain relative.
  No absolute reset timestamp is inferred.
- Refresh is manual only. Keep the authenticated OpenDesign Plan & balance
  page open in Chrome, then select Refresh in either detail view.
- Failed reads keep the last complete pair with its original observation time
  and an explicit stale/error state. Missing data stays unavailable.
- The existing read-only UIA collector is reused. There is no background
  browser interaction, credential extraction or private API polling.

Existing provider behavior and the signed updater configuration are preserved.
