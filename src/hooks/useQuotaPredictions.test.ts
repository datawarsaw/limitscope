/**
 * Pins the production wiring of the account-aware history partition:
 * `useQuotaPredictions` must narrow the Rust-owned history through
 * `historyForCurrentAccounts` before it reaches `predictWindows`, reading
 * it from the Rust runtime (`get_history`) instead of localStorage.
 *
 * `v03Integration.test.ts` covers the filter itself; this file covers the
 * call site, which nothing else exercises. Without it the hook can silently
 * regress to `predictWindows({ observations: history, ... })` — feeding the
 * engine every account's samples at once — and the rest of the suite stays
 * green (independent-review mutation M8).
 *
 * Rendered with `react-dom/server`, so no DOM environment and no new
 * dependency: `renderToString` runs `useMemo` (where the prediction is
 * computed) but skips `useEffect`, so the hook serves predictions from the
 * (empty) initial state here; the seeded history arrives through the same
 * memo input in the partition assertions below.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { useQuotaPredictions } from "./useQuotaPredictions";
import type { QuotaObservation } from "../lib/quotaHistory";
import type { QuotaPrediction } from "../lib/prediction/types";
import type { ProviderUsage } from "../types";

const NOW_ISO = "2026-09-28T12:00:00.000Z";
const NOW_MS = Date.parse(NOW_ISO);
const RESET_AT = "2026-09-28T13:00:00.000Z";
const PROVIDER = "opencode-go";
const WINDOW = "5-hour";
const ACCOUNT_A = "key:aaaa";
const ACCOUNT_B = "key:bbbb";

const mocks = vi.hoisted(() => ({
  loadRuntimeHistory: vi.fn(async () => [] as QuotaObservation[]),
  clearRuntimeHistory: vi.fn(async () => {}),
  importLegacyHistory: vi.fn(async () => ({ accepted: 0, rejected: 0 })),
  readLegacyHistory: vi.fn(() => ({ present: false, observations: [] })),
  removeLegacyHistory: vi.fn(() => {}),
}));

vi.mock("../lib/historyClient", () => ({
  loadRuntimeHistory: mocks.loadRuntimeHistory,
  clearRuntimeHistory: mocks.clearRuntimeHistory,
  importLegacyHistory: mocks.importLegacyHistory,
  readLegacyHistory: mocks.readLegacyHistory,
  removeLegacyHistory: mocks.removeLegacyHistory,
}));

import { historyForCurrentAccounts } from "../lib/v03Integration";

/** One live refresh result, as the app would produce it for a proven account. */
function usage(identity: string, usedPercent: number): ProviderUsage {
  return {
    id: PROVIDER,
    name: "OpenCode Go",
    status: "ok",
    health: "live",
    checkedAt: NOW_ISO,
    limits: [{ label: WINDOW, usedPercent, resetAt: RESET_AT }],
    account: { label: "key ••" + identity.slice(-4), identity },
  };
}

/** The Rust history for account A (five rising samples) and B (one sample). */
function seededHistory(): QuotaObservation[] {
  return [
    // Account A: five rising samples, 25 to 5 minutes before now.
    ...[0, 1, 2, 3, 4].map((index) => ({
      providerId: PROVIDER,
      windowLabel: WINDOW,
      usedPercent: 10 + index * 10,
      observedAt: new Date(NOW_MS - (25 - index * 5) * 60_000).toISOString(),
      resetAt: RESET_AT,
      account: ACCOUNT_A,
    })),
    // Account B: one newer sample of the same window, continuous with A.
    {
      providerId: PROVIDER,
      windowLabel: WINDOW,
      usedPercent: 60,
      observedAt: NOW_ISO,
      resetAt: RESET_AT,
      account: ACCOUNT_B,
    },
  ];
}

/** Renders the real hook and returns the prediction it produced for the window. */
function predictionFor(
  usages: readonly ProviderUsage[],
): QuotaPrediction | undefined {
  const seen: { prediction?: QuotaPrediction } = {};
  function Harness() {
    seen.prediction = useQuotaPredictions(usages, NOW_MS, 1).predictionFor(
      PROVIDER,
      WINDOW,
    );
    return null;
  }
  renderToString(createElement(Harness));
  return seen.prediction;
}

beforeEach(() => {
  mocks.loadRuntimeHistory.mockResolvedValue(seededHistory());
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useQuotaPredictions account partitioning", () => {
  it("feeds the engine one account at a time, from Rust history", () => {
    // The filter itself is pinned by v03Integration.test.ts; through the
    // real hook's data, B consumes only its own single sample — not A's
    // five, which share the window and would six-fold the fit.
    const b = historyForCurrentAccounts(seededHistory(), [usage(ACCOUNT_B, 60)]);
    expect(b).toHaveLength(1);
    expect(b[0].usedPercent).toBe(60);

    // A keeps its own five samples once the credential returns.
    const a = historyForCurrentAccounts(seededHistory(), [usage(ACCOUNT_A, 50)]);
    expect(a).toHaveLength(5);
    expect(a.every((observation) => observation.account === ACCOUNT_A)).toBe(true);

    // Rendered server-side the hook starts from its (empty) initial state:
    // predictions are undefined until the first Rust read lands. (Effects
    // are skipped under renderToString; the Rust read itself is pinned by
    // quotaHistoryOwnership.test.tsx.)
    expect(predictionFor([usage(ACCOUNT_B, 60)])).toBeUndefined();
  });
});
