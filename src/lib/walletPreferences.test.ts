import { describe, expect, it } from "vitest";
import {
  DEFAULT_WALLET_PREFERENCES,
  isWalletInFloatingBar,
  isWalletMonitored,
  sanitizeWalletPreferences,
  walletDefaultsFor,
  walletRegistryNote,
  withWalletFloatingBar,
  withWalletMonitored,
} from "./walletPreferences";

describe("wallet preferences", () => {
  it("defaults real collectors to monitored and visible", () => {
    expect(walletDefaultsFor("openrouter")).toEqual({
      monitored: true,
      floatingBar: true,
    });
    expect(walletDefaultsFor("deepseek")).toEqual({
      monitored: true,
      floatingBar: true,
    });
    expect(isWalletMonitored(DEFAULT_WALLET_PREFERENCES, "openrouter")).toBe(
      true,
    );
    expect(
      isWalletInFloatingBar(DEFAULT_WALLET_PREFERENCES, "deepseek"),
    ).toBe(true);
    // No preferences object at all resolves the same way: existing users
    // and new installs keep today's behavior.
    expect(isWalletMonitored(null, "openrouter")).toBe(true);
    expect(isWalletInFloatingBar(undefined, "openrouter")).toBe(true);
  });

  it("defaults the registered OpenAI placeholder to unmonitored and hidden", () => {
    expect(walletDefaultsFor("openai")).toEqual({
      monitored: false,
      floatingBar: false,
    });
    expect(isWalletMonitored(DEFAULT_WALLET_PREFERENCES, "openai")).toBe(
      false,
    );
    expect(
      isWalletInFloatingBar(DEFAULT_WALLET_PREFERENCES, "openai"),
    ).toBe(false);
  });

  it("an explicit flag always wins over the default", () => {
    const prefs = sanitizeWalletPreferences({
      monitored: { openai: true },
      floatingBar: { openrouter: false },
    });
    expect(isWalletMonitored(prefs, "openai")).toBe(true);
    expect(isWalletInFloatingBar(prefs, "openrouter")).toBe(false);
  });

  it("sanitizes persisted shapes into flag records", () => {
    expect(sanitizeWalletPreferences(null)).toEqual(
      DEFAULT_WALLET_PREFERENCES,
    );
    expect(sanitizeWalletPreferences("garbage")).toEqual(
      DEFAULT_WALLET_PREFERENCES,
    );
    expect(
      sanitizeWalletPreferences({
        monitored: { openrouter: false, deepseek: "yes", "": true },
        floatingBar: ["deepseek"],
      }),
    ).toEqual({
      monitored: { openrouter: false },
      floatingBar: {},
    });
  });

  it("setting a flag is a pure update that leaves sibling records alone", () => {
    const base = sanitizeWalletPreferences({
      monitored: { deepseek: false },
      floatingBar: { openrouter: false },
    });
    const next = withWalletMonitored(base, "openrouter", false);
    expect(next.monitored).toEqual({ deepseek: false, openrouter: false });
    expect(next.floatingBar).toEqual({ openrouter: false });
    expect(base.monitored).toEqual({ deepseek: false });

    const shown = withWalletFloatingBar(next, "openrouter", true);
    expect(shown.floatingBar).toEqual({ openrouter: true });
    expect(shown.monitored).toEqual({ deepseek: false, openrouter: false });
  });

  it("setting the same value again returns the same object", () => {
    const base = sanitizeWalletPreferences({
      monitored: { openrouter: false },
      floatingBar: {},
    });
    expect(withWalletMonitored(base, "openrouter", false)).toBe(base);
    expect(withWalletFloatingBar(base, "openrouter", true)).not.toBe(base);
    expect(withWalletFloatingBar(base, "deepseek", true)).not.toBe(base);
  });

  it("states why the OpenAI placeholder cannot be monitored", () => {
    const note = walletRegistryNote("openai");
    expect(note).toContain("usage and cost reporting");
    expect(note).toContain("not a prepaid remaining balance");
    expect(walletRegistryNote("openrouter")).toBeNull();
    expect(walletRegistryNote("deepseek")).toBeNull();
  });
});
