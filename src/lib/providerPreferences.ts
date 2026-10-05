/**
 * Provider presentation preferences (v0.6) - a UI-only preference layer.
 *
 * A hidden provider means "do not show prominently in the UI". It does
 * NOT mean "do not fetch", "disable", or "remove from the runtime":
 * the Rust runtime keeps refreshing every registered provider, history
 * keeps accumulating, and notification evaluation is unchanged. Ordering
 * affects presentation surfaces only (main provider rail, floating bar).
 *
 * Identity is always the stable provider id (e.g. "openai-codex"), never
 * the display name.
 */

export type ProviderPreferences = {
  /** Preferred display order of known provider ids. */
  order: string[];
  /** Ids the user chose to hide from prominent UI surfaces. */
  hidden: string[];
};

export const DEFAULT_PROVIDER_PREFERENCES: ProviderPreferences = {
  order: [],
  hidden: [],
};

export type ResolvedProviderPreferences = {
  /** Registry ids in display order, minus hidden ids. */
  visibleOrderedIds: string[];
  /** Hidden ids that still exist in the registry, in display order. */
  hiddenIds: string[];
};

function isProviderId(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function cleanIdList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const entry of value) {
    if (!isProviderId(entry)) continue;
    if (seen.has(entry)) continue;
    seen.add(entry);
    out.push(entry);
  }
  return out;
}

/**
 * Tolerantly parses persisted preferences. Unknown shapes, malformed ids,
 * and duplicates collapse to safe defaults instead of failing: existing
 * users without preferences get an empty order/hidden pair, which resolves
 * to canonical registry order with nothing hidden.
 */
export function sanitizeProviderPreferences(
  value: unknown,
): ProviderPreferences {
  if (!value || typeof value !== "object") {
    return { ...DEFAULT_PROVIDER_PREFERENCES };
  }
  const record = value as Partial<ProviderPreferences>;
  return {
    order: cleanIdList(record.order),
    hidden: cleanIdList(record.hidden),
  };
}

/**
 * Resolves display order from the runtime registry order plus saved
 * preferences. Deterministic rules: saved order entries that no longer
 * exist are ignored; registry ids absent from the saved order (e.g. a
 * provider added in a future version) are appended in registry order
 * after known entries. With no saved preferences the resolved order IS
 * the registry order.
 */
export function resolveProviderPreferences(
  registryIds: readonly string[],
  saved: unknown,
): ResolvedProviderPreferences {
  const prefs = sanitizeProviderPreferences(saved);
  const registry = registryIds.filter(isProviderId);
  const inRegistry = new Set(registry);
  const hiddenSet = new Set(
    prefs.hidden.filter((id) => inRegistry.has(id)),
  );

  const ordered: string[] = [];
  const seen = new Set<string>();
  for (const id of prefs.order) {
    if (!inRegistry.has(id) || seen.has(id)) continue;
    seen.add(id);
    ordered.push(id);
  }
  for (const id of registry) {
    if (seen.has(id)) continue;
    seen.add(id);
    ordered.push(id);
  }

  // At-least-one-visible rule (v0.6 settings remediation): toggle
  // refusal keeps the interactive path from creating an empty UI, so
  // an all-hidden store here can only come from a corrupt or stale
  // write - heal it instead of rendering nothing.
  if (ordered.length > 0 && ordered.every((id) => hiddenSet.has(id))) {
    hiddenSet.delete(ordered[0]);
  }

  return {
    visibleOrderedIds: ordered.filter((id) => !hiddenSet.has(id)),
    hiddenIds: ordered.filter((id) => hiddenSet.has(id)),
  };
}

/**
 * Orders any provider-keyed list for presentation: visible providers in
 * preference order, hidden providers removed. The single shared helper so
 * main and floating windows cannot diverge. Item identity is preserved -
 * the returned array holds the same object references (history,
 * prediction, and notification contexts ride along untouched).
 */
export function orderVisibleProviders<T>(
  items: readonly T[],
  idOf: (item: T) => string,
  registryIds: readonly string[],
  saved: unknown,
): T[] {
  const { visibleOrderedIds } = resolveProviderPreferences(registryIds, saved);
  const rank = new Map(visibleOrderedIds.map((id, index) => [id, index]));
  const seen = new Set<string>();
  const visible: T[] = [];
  for (const item of items) {
    const id = idOf(item);
    if (!rank.has(id) || seen.has(id)) continue;
    seen.add(id);
    visible.push(item);
  }
  visible.sort((a, b) => rank.get(idOf(a))! - rank.get(idOf(b))!);
  return visible;
}

/**
 * Full display order including hidden providers (the settings list shows
 * every registered provider with its visibility state).
 */
export function fullDisplayOrder(
  registryIds: readonly string[],
  saved: unknown,
): string[] {
  const prefs = sanitizeProviderPreferences(saved);
  const { visibleOrderedIds, hiddenIds } = resolveProviderPreferences(
    registryIds,
    prefs,
  );
  const orderIndex = new Map(prefs.order.map((id, index) => [id, index]));
  const all = [...visibleOrderedIds, ...hiddenIds];
  const rankOf = (id: string): number =>
    orderIndex.has(id) ? orderIndex.get(id)! : Number.MAX_SAFE_INTEGER;
  all.sort((a, b) => {
    const ra = rankOf(a);
    const rb = rankOf(b);
    if (ra !== rb) return ra - rb;
    return registryIds.indexOf(a) - registryIds.indexOf(b);
  });
  return all;
}

/**
 * Whether hiding providerId is allowed. The final visible provider
 * cannot be hidden - preventing an accidentally unusable UI. Returns
 * false when the provider is unknown or already hidden.
 */
export function canHideProvider(
  registryIds: readonly string[],
  saved: unknown,
  providerId: string,
): boolean {
  const { visibleOrderedIds } = resolveProviderPreferences(registryIds, saved);
  if (!visibleOrderedIds.includes(providerId)) return false;
  return visibleOrderedIds.length > 1;
}

/**
 * Moves a provider within the full display order. Out-of-range moves
 * (first up, last down) and unknown ids return the order unchanged.
 * The returned array is the new persisted order value.
 */
export function moveProviderOrder(
  registryIds: readonly string[],
  saved: unknown,
  providerId: string,
  direction: 1 | -1,
): string[] {
  const all = fullDisplayOrder(registryIds, saved);
  const index = all.indexOf(providerId);
  if (index === -1) return all;
  const next = index + direction;
  if (next < 0 || next >= all.length) return all;
  const moved = [...all];
  const tmp = moved[index];
  moved[index] = moved[next];
  moved[next] = tmp;
  return moved;
}

/**
 * Toggles visibility. Hiding the final visible provider is refused
 * (applied: false, preferences unchanged) so the caller can explain
 * instead of leaving an empty UI. Showing a provider always applies.
 */
export function toggleProviderHidden(
  registryIds: readonly string[],
  saved: unknown,
  providerId: string,
  hidden: boolean,
): { prefs: ProviderPreferences; applied: boolean } {
  const prefs = sanitizeProviderPreferences(saved);
  const inRegistry = new Set(registryIds.filter(isProviderId));
  if (!inRegistry.has(providerId)) {
    return { prefs, applied: false };
  }
  const hiddenSet = new Set(
    prefs.hidden.filter((id) => inRegistry.has(id)),
  );
  if (hidden) {
    if (!canHideProvider(registryIds, prefs, providerId)) {
      return { prefs, applied: false };
    }
    hiddenSet.add(providerId);
  } else {
    hiddenSet.delete(providerId);
  }
  return { prefs: { order: prefs.order, hidden: [...hiddenSet] }, applied: true };
}

