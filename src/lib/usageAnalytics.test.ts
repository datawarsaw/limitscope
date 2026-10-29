// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async () => ({}) as unknown),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { codexScopeIsCurrentOrNewer, loadUsageAnalytics } from "./usageAnalytics";

describe("usageAnalytics client contract", () => {
  beforeEach(() => {
    invoke.mockClear();
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("does not fabricate an analytics result outside Tauri", async () => {
    await expect(
      loadUsageAnalytics({ range: "24h" }),
    ).rejects.toThrow("Usage analytics requires the Tauri runtime");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("invokes one get_usage_analytics command with the normalized query", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const response = { schemaVersion: 1 };
    invoke.mockResolvedValueOnce(response);

    await expect(
      loadUsageAnalytics({
        range: "7d",
        providerId: "zai",
        account: null,
        windowLabel: "5-hour",
        exactAccount: true,
      }),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("get_usage_analytics", {
      query: {
        range: "7d",
        providerId: "zai",
        account: null,
        windowLabel: "5-hour",
        exactAccount: true,
      },
    });
  });
});

describe("the Codex identity scope fence", () => {
  const slot = (providerId: string, generation: number, identity: string | null) => ({
    providerId,
    generation,
    identity,
  });

  it("accepts a response computed under the current session", () => {
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 5, "user-a")],
        [slot("openai-codex", 5, "user-a")],
      ),
    ).toBe(true);
  });

  it("accepts a response computed under a NEWER session", () => {
    // The backend observed the session move before the consumer did.
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 6, "user-b")],
        [slot("openai-codex", 5, "user-a")],
      ),
    ).toBe(true);
  });

  it("rejects a response computed under an older session", () => {
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 5, "user-a")],
        [slot("openai-codex", 6, "user-b")],
      ),
    ).toBe(false);
  });

  it("rejects a response whose identity differs at the same generation", () => {
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 5, "user-b")],
        [slot("openai-codex", 5, "user-a")],
      ),
    ).toBe(false);
  });

  it("rejects a response that carries no scope while a slot is expected", () => {
    expect(codexScopeIsCurrentOrNewer(undefined, [slot("openai-codex", 5, "user-a")])).toBe(false);
    expect(codexScopeIsCurrentOrNewer(undefined, [])).toBe(true);
  });

  it("rejects a response that does not carry an expected slot", () => {
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex:secondary", 3, "user-b")],
        [
          slot("openai-codex", 5, "user-a"),
          slot("openai-codex:secondary", 3, "user-b"),
        ],
      ),
    ).toBe(false);
  });

  it("keeps one slot's failure from suppressing the other slot", () => {
    // The primary moved; the secondary did not. Only the primary's rows
    // are in question, and the secondary is unaffected.
    expect(
      codexScopeIsCurrentOrNewer(
        [
          slot("openai-codex", 6, "user-b"),
          slot("openai-codex:secondary", 3, "user-c"),
        ],
        [
          slot("openai-codex", 5, "user-a"),
          slot("openai-codex:secondary", 3, "user-c"),
        ],
      ),
    ).toBe(true);
  });

  it("follows an A to B to A move by its generation, not by its identity", () => {
    // A and back to A: the identity is the same, but the generation moved
    // twice, so the response belongs to an older session.
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 5, "user-a")],
        [slot("openai-codex", 7, "user-a")],
      ),
    ).toBe(false);
    expect(
      codexScopeIsCurrentOrNewer(
        [slot("openai-codex", 7, "user-a")],
        [slot("openai-codex", 7, "user-a")],
      ),
    ).toBe(true);
  });
});
