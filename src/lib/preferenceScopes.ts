/**
 * The preference groups whose saved state the app can fail to verify.
 *
 * A setter may persist the value and reject afterwards, so the last value
 * this WebView saw is not evidence of what Rust stored. When the follow-up
 * read fails as well, the scope stays explicitly unknown: the UI says so
 * instead of presenting a stale or merely intended value as the saved one.
 */
export type PreferenceScope =
  | "launchAtStartup"
  | "quotaNotifications"
  | "walletMonitoring"
  | "refreshIntervalMinutes"
  | "codexAccounts"
  | "usageIntelligence";

export const ALL_PREFERENCE_SCOPES: readonly PreferenceScope[] = [
  "launchAtStartup",
  "quotaNotifications",
  "walletMonitoring",
  "refreshIntervalMinutes",
  "codexAccounts",
  "usageIntelligence",
];

export const PREFERENCE_SCOPE_LABELS: Record<PreferenceScope, string> = {
  launchAtStartup: "launch at startup",
  quotaNotifications: "quota notifications",
  walletMonitoring: "wallet monitoring",
  refreshIntervalMinutes: "refresh interval",
  codexAccounts: "Codex accounts",
  usageIntelligence: "Usage Intelligence",
};

/** The visible note for a scope whose saved state could not be verified. */
export function preferenceScopeUnverifiedNote(scope: PreferenceScope): string {
  return `The saved state of ${PREFERENCE_SCOPE_LABELS[scope]} could not be verified.`;
}
