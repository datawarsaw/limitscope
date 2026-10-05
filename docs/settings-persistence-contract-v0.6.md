# LimitScope v0.6 Settings Persistence Contract

Status: **ACTIVE CONTRACT** (v0.6 remediation)
Branch: `fix/v0.6-settings-persistence` (base: `integration/v0.6-core-rehearsal` @ `b0978b9`)
Provenance: `research/v0.6-settings-migration-audit` — its defects and required behaviors are pinned by tests in `src/lib/settings.test.ts`, `src/lib/floatingWindowPrefs.test.ts`, `src/lib/floatingWindowChrome.test.ts`, and `src/hooks/useSettings.test.tsx`, with fixtures under `fixtures/settings-migration/`.

---

## 1. Storage keys (unchanged)

| Key | Owner | Contents |
|---|---|---|
| `rate-limits.settings.v1` | `src/lib/settings.ts` | Core settings JSON object |
| `rate-limits.floating-quota.v1` | `src/lib/floatingWindowPrefs.ts` | Floating bar preferences JSON object |

No version bump was needed. Both v1 keys evolve in place; every change below is backward- and forward-compatible with v0.3/v0.5 payloads.

## 2. Canonical known fields

### `rate-limits.settings.v1`

```typescript
type Settings = {
  launchAtStartup: boolean;                  // default false
  refreshIntervalMinutes: 1 | 5 | 15 | 30;   // default 5
  theme: "graphite" | "glass" | "oled";      // default "graphite"
  quotaNotifications: boolean;               // default false
  providerPreferences: {                     // default { order: [], hidden: [] }
    order: string[];                         // preferred display order of provider ids
    hidden: string[];                        // ids hidden from prominent UI surfaces
  };
  quotaPerspective: "used" | "remaining";    // default "used"
};
```

Notes:

- `providerPreferences` arrives from the accepted `feature/v0.6-provider-preferences` contract (`src/lib/providerPreferences.ts`). Hidden is presentation-only: the Rust runtime keeps refreshing every registered provider, history keeps accumulating, and notification evaluation is unchanged.
- `quotaPerspective` arrives from the accepted `feature/v0.6-quota-perspective` contract (`src/lib/quotaPresentation.ts`). It is presentation-only and never mutates canonical `usedPercent` or history values.
- `surfaceTransparency` is **not** a canonical field (transparency is not implemented). If present in stored JSON it is treated as a foreign field and preserved verbatim (see §3). Do not "adopt" it into the schema until the feature ships.

### `rate-limits.floating-quota.v1`

```typescript
type FloatingQuotaPrefs = {
  visible: boolean;             // default true  (window lifecycle state)
  floatingBarEnabled: boolean;  // default true  (feature switch, v0.6)
  alwaysOnTop: boolean;         // default true  (pin)
  x: number | null;             // default null  (physical px; null = system placed)
  y: number | null;             // default null  (physical px)
};
```

### Hidden vs disabled (floating bar)

- **`visible`** is window lifecycle state: tray/context-menu show and hide. Hiding must never change `floatingBarEnabled` (native acceptance Flow 9).
- **`floatingBarEnabled`** is the feature switch. Disabling sets it to `false` and must not erase `x`, `y`, or `alwaysOnTop`; re-enabling restores the bar with the prior geometry and pin (native acceptance Flow 11). `setFloatingBarEnabled()` in `src/lib/floatingWindowChrome.ts` is the single writer of the flag; tray/settings gating on the value lands with the floating enable/disable UI lane.

## 3. Unknown-field retention policy (safe write contract)

Both storage modules use a **read-modify-write merge** (`mergeSettingsWithRaw` / `mergeFloatingPrefsWithRaw`):

1. Read the raw stored value and parse it.
2. If it is not valid JSON, or parses to a non-object (array, primitive, `null`), **preserve nothing** — fall back to canonical defaults. Garbage is not carried forward.
3. Copy every key the canonical schema does not own (foreign/future fields) verbatim into the new object.
4. Overlay the validated in-memory canonical fields on top of their known keys.
5. Write the merged object.

Consequences:

- **Unknown fields survive every save.** A field another lane (or a future version) wrote is never erased by a save from this schema. Unknown nested objects are preserved by value.
- **Canonical fields always win.** A stale raw value for a known key — including an invalid one such as `theme: "broken"` — is overwritten by the validated in-memory value on save; it is never preserved merely because it existed in raw JSON.
- Writes never adopt unknown values into the typed model; they only avoid destroying them.

Example (`rate-limits.settings.v1`):

```jsonc
// stored raw
{ "theme": "broken", "futureFeature": 123 }
// after canonical load + save (theme validated/normalized, foreign field kept)
{ "futureFeature": 123, "launchAtStartup": false, "refreshIntervalMinutes": 5,
  "theme": "graphite", "quotaNotifications": false,
  "providerPreferences": { "order": [], "hidden": [] }, "quotaPerspective": "used" }
```

## 4. Invalid-value behavior (load)

`parseSettings` / `parseFloatingQuotaPrefs` validate field-by-field; an invalid field falls back to its default while valid siblings survive:

| Input | Result |
|---|---|
| Corrupt / truncated JSON root | Full canonical defaults |
| Root is an array or primitive | Full canonical defaults |
| `theme: "cyberpunk-neon"` | `theme: "graphite"` |
| `refreshIntervalMinutes: 42` | `refreshIntervalMinutes: 5` |
| `launchAtStartup` / `quotaNotifications` non-boolean | `false` |
| `quotaPerspective: "inverted"` | `"used"` |
| `providerPreferences` non-object / `order` / `hidden` non-array | `{ order: [], hidden: [] }` |
| Provider id entries that are non-strings, empty, or duplicated | Stripped / deduplicated (first occurrence wins) |
| `floatingBarEnabled` absent or non-boolean | `true` |
| `x` / `y` non-finite or wrong type | `null` (system placed) |

## 5. Legacy migration defaults

| Stored payload | Behavior |
|---|---|
| v0.3 minimal (`launchAtStartup`, `refreshIntervalMinutes` only) | Loads as-is; `theme`, `quotaNotifications`, `providerPreferences`, `quotaPerspective` default |
| v0.5 full (adds `theme`, `quotaNotifications`) | Loads as-is; `providerPreferences`, `quotaPerspective` default |
| Provider-preferences-only payload (older parallel lane) | Loads; `quotaPerspective` defaults to `"used"` |
| Quota-perspective-only payload (older parallel lane) | Loads; `providerPreferences` defaults to `{ order: [], hidden: [] }` |
| Both fields present | Loads as-is |
| Floating prefs without `floatingBarEnabled` | Feature treated as enabled (`true`) — legacy users keep the bar |
| Floating prefs with `floatingBarEnabled: false` | Disabled, geometry/pin retained |

Provider order resolution (`resolveProviderPreferences`):

- Saved order entries not in the runtime registry are ignored (removed providers are safe).
- Registry providers missing from the saved order are appended in registry order (new providers appear automatically).
- Duplicates collapse to the first occurrence.
- **At-least-one-visible rule**: interactive hiding is refused for the last visible provider (`toggleProviderHidden` returns `applied: false`); an all-hidden stored state (corrupt/stale write) self-heals by un-hiding the first provider in display order. This self-heal is a deliberate v0.6 remediation addition on top of the accepted lane module.

## 6. Future schema-extension rule

When adding a settings field in v0.7+:

1. Add the field to the canonical `Settings` (or `FloatingQuotaPrefs`) type, `DEFAULT_SETTINGS`, the parser, and the **known-keys set** in the same module.
2. Give it a default that matches legacy behavior for storage without the key.
3. Add a migration fixture plus roundtrip, invalid-value, and cross-feature-survival tests.

Until a field is added to the known-keys set, saves preserve it as a foreign field — which is exactly what makes out-of-order lane integration safe. Never persist a whole typed object with `JSON.stringify(settings)` directly; always go through `saveSettings` / `saveFloatingQuotaPrefs` (or the exported merge helpers) so foreign fields survive.

## 7. Test coverage map (required contract assertions)

| # | Assertion | Test |
|---|---|---|
| 1 | v0.3 load | `settings.test.ts` "loads legacy v0.3 settings…" |
| 2 | v0.5 load | `settings.test.ts` "loads legacy v0.5 settings…" |
| 3 | `providerPreferences` survives `quotaPerspective` save | `settings.test.ts` "keeps providerPreferences…" |
| 4 | `quotaPerspective` survives `providerPreferences` save | `settings.test.ts` "keeps quotaPerspective…" |
| 5 | Unknown future field survives save | `settings.test.ts` "keeps unknown future fields…"; merge contract tests |
| 6 | Invalid known value normalizes | `settings.test.ts` "normalizes invalid known values…" |
| 7 | Corrupt JSON safely defaults | `settings.test.ts` corrupt fixtures; `floatingWindowPrefs.test.ts` |
| 8 | Provider order migration | `settings.test.ts` "appends registry providers…" |
| 9 | At least one visible | `settings.test.ts` "never hides the last visible provider…" |
| 10 | Floating enabled default | `floatingWindowPrefs.test.ts` "defaults floatingBarEnabled…" |
| 11 | Floating disabled survives drag save | `floatingWindowPrefs.test.ts` "keeps the disabled flag…" |
| 12 | Floating geometry survives disable/enable | `floatingWindowPrefs.test.ts`; `floatingWindowChrome.test.ts` |
| 13 | Pin survives disable/enable | same as 12 |
| 14 | Hidden ≠ disabled | `floatingWindowPrefs.test.ts` "treats hidden and disabled…" |
| 15 | Full canonical roundtrip | `settings.test.ts` "round-trips the full v0.6 canonical settings" |

Cross-feature survival is additionally pinned at the React layer in `useSettings.test.tsx` (theme change preserves v0.6 and foreign fields; perspective and provider-preference setters persist).
