import { describe, expect, it } from "vitest";
import { predictWindows } from "./prediction/engine";
import type { QuotaObservation } from "./quotaHistory";
import { historyForCurrentAccounts } from "./v03Integration";
import type { ProviderUsage } from "../types";

/**
 * Regression coverage for MIC-298: pressing Refresh could unmount the whole
 * dashboard (black window). The crash was not a provider failure — it fired
 * while rendering predictions from freshly recorded history, so the chain
 * under test is the TS half of the refresh pipeline as it runs since v0.5
 * phase 2: history is recorded by the Rust runtime (its recording rules and
 * eligibility live in src-tauri/src/history.rs tests), and the TS side
 * receives a history read result, narrows it to the current accounts, and
 * predicts — here with the exact crashing history shape.
 */

function goodUsage(
  id: string,
  name: string,
  usedPercent: number,
  checkedAt: string,
  resetAt: string,
): ProviderUsage {
  return {
    id,
    name,
    status: "ok",
    health: "live",
    checkedAt,
    limits: [{ label: "5-hour", usedPercent, resetAt }],
  };
}

/** What the runtime delivers for a provider whose fetch failed outright. */
function failedUsage(id: string, name: string, error: string): ProviderUsage {
  return {
    id,
    name,
    status: "error",
    health: "error",
    checkedAt: "2026-09-28T12:00:00.000Z",
    limits: [],
    error,
  };
}

/** A stored observation, exactly as `get_history` returns it. */
function stored(
  providerId: string,
  windowLabel: string,
  usedPercent: number,
  observedAt: string,
  resetAt?: string,
): QuotaObservation {
  return {
    providerId,
    windowLabel,
    usedPercent,
    observedAt,
    ...(resetAt ? { resetAt } : {}),
  };
}

const REFRESH_AT = "2026-09-28T12:00:00.000Z";
const RESET_AT = "2026-09-28T14:00:00.000Z";

describe("refresh pipeline (MIC-298 regression)", () => {
  it("completes a fully successful snapshot and produces predictions", () => {
    // The Rust recorder stored one sample per provider (source-truth shape).
    const history: QuotaObservation[] = [
      stored("openai-codex", "5-hour", 42.5, REFRESH_AT, RESET_AT),
      stored("zai", "5-hour", 63, REFRESH_AT, RESET_AT),
    ];
    const usages = [
      goodUsage("openai-codex", "OpenAI / Codex", 42.5, REFRESH_AT, RESET_AT),
      goodUsage("zai", "Z.ai", 63, REFRESH_AT, RESET_AT),
    ];
    const current = historyForCurrentAccounts(history, usages);
    expect(current).toHaveLength(2);
    expect(() =>
      predictWindows({ observations: current, now: REFRESH_AT }),
    ).not.toThrow();
  });

  it("isolates one failed provider and keeps the rest of the pipeline working", () => {
    // A failed provider never enters Rust history, so the read result
    // contains only the good provider's samples.
    const history: QuotaObservation[] = [
      stored("openai-codex", "5-hour", 42.5, REFRESH_AT, RESET_AT),
    ];
    const usages = [
      goodUsage("openai-codex", "OpenAI / Codex", 42.5, REFRESH_AT, RESET_AT),
      failedUsage("zai", "Z.ai", "Refresh failed: key rejected (auth_invalid)"),
    ];
    const failed = usages.find((u) => u.id === "zai")!;
    expect(failed.status).toBe("error");
    expect(failed.error).toContain("auth_invalid");
    expect(failed.limits).toHaveLength(0);
    const good = usages.find((u) => u.id === "openai-codex")!;
    expect(good.status).toBe("ok");
    expect(good.limits).toHaveLength(1);
    const current = historyForCurrentAccounts(history, usages);
    expect(current.every((o) => o.providerId === "openai-codex")).toBe(true);
    expect(() =>
      predictWindows({ observations: current, now: REFRESH_AT }),
    ).not.toThrow();
  });

  it("replays the crashing idle-provider history without unmounting-grade failure", () => {
    // The exact field crash: Z.ai "5-hour" at a constant 63% whose resetAt
    // jumped forward mid-session; the newest 3-sample segment fit resolved to
    // a tiny positive float-noise burn rate (see the engine regression tests).
    const zaiSamples: readonly (readonly [string, string])[] = [
      ["2026-09-28T11:56:29.984Z", "2026-09-28T14:54:58.990Z"],
      ["2026-09-28T11:56:50.675Z", "2026-09-28T14:54:58.990Z"],
      ["2026-09-28T11:57:36.392Z", "2026-09-28T14:54:58.990Z"],
      ["2026-09-28T11:57:36.982Z", "2026-09-28T14:54:58.990Z"],
      ["2026-09-28T11:59:28.203Z", "2026-09-28T14:59:12.278Z"],
      ["2026-09-28T11:59:47.138Z", "2026-09-28T14:59:12.278Z"],
      ["2026-09-28T12:00:53.589Z", "2026-09-28T14:59:12.278Z"],
    ];
    const history = zaiSamples.map(([observedAt, resetAt]) =>
      stored("zai", "5-hour", 63, observedAt, resetAt),
    );
    const crashNow = "2026-09-28T12:01:30.000Z";
    let predictions: ReturnType<typeof predictWindows>;
    expect(() => {
      predictions = predictWindows({ observations: history, now: crashNow });
    }).not.toThrow();
    const zai = predictions!.find((p) => p.providerId === "zai")!;
    expect(zai).toBeDefined();
    expect(zai.estimatedExhaustionAt).toBeUndefined();
  });

  it("survives repeated snapshots of an idle provider across a reset change", () => {
    // Five successive runtime cycles; each history read result carries one
    // more stored sample (same-instant duplicates deduped in Rust).
    const base = Date.parse("2026-09-28T11:50:00.000Z");
    const resets = [
      "2026-09-28T14:54:58.990Z",
      "2026-09-28T14:54:58.990Z",
      "2026-09-28T14:59:12.278Z",
      "2026-09-28T14:59:12.278Z",
      "2026-09-28T14:59:12.278Z",
    ] as const;
    let history: QuotaObservation[] = [];
    for (let cycle = 0; cycle < resets.length; cycle++) {
      const now = new Date(base + cycle * 60_000).toISOString();
      history = [...history, stored("zai", "5-hour", 63, now, resets[cycle])];
      const usages = [goodUsage("zai", "Z.ai", 63, now, resets[cycle])];
      const current = historyForCurrentAccounts(history, usages);
      expect(current).toHaveLength(cycle + 1);
      expect(() => predictWindows({ observations: current, now })).not.toThrow();
    }
    expect(history).toHaveLength(5);
  });
});
