import { describe, expect, it } from "vitest";
import {
  DEFAULT_CODEX_ACCOUNT_PREFERENCES,
  codexAccountSettings,
  isCodexAccountConfigured,
  sanitizeCodexAccountPreferences,
  withCodexAccount,
  type CodexAccountSettings,
} from "./codexAccounts";

const PRIMARY = "openai-codex";
const SECONDARY = "openai-codex:secondary";

describe("codex account preferences", () => {
  it("defaults to two monitored accounts with no alias or home", () => {
    expect(DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts).toEqual([
      { accountId: PRIMARY, alias: "", codexHome: "", monitored: true },
      { accountId: SECONDARY, alias: "", codexHome: "", monitored: true },
    ]);
  });

  it("falls back to the defaults for undefined and garbage input", () => {
    expect(sanitizeCodexAccountPreferences(undefined)).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES);
    expect(sanitizeCodexAccountPreferences("nope")).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES);
    expect(sanitizeCodexAccountPreferences([1, 2])).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES);
    expect(sanitizeCodexAccountPreferences({ accounts: "x" })).toEqual(
      DEFAULT_CODEX_ACCOUNT_PREFERENCES,
    );
  });

  it("keeps a valid list as stored", () => {
    const stored = {
      accounts: [
        { accountId: PRIMARY, alias: "Work", codexHome: "C:\\codex\\a", monitored: false },
        { accountId: SECONDARY, alias: "Home", codexHome: "C:\\codex\\b", monitored: true },
      ],
    };
    expect(sanitizeCodexAccountPreferences(stored)).toEqual(stored);
  });

  it("drops an unknown account id", () => {
    const result = sanitizeCodexAccountPreferences({
      accounts: [
        { accountId: "acme:corp", alias: "X", codexHome: "/x", monitored: false },
        { accountId: SECONDARY, alias: "Home", codexHome: "/b", monitored: false },
      ],
    });
    expect(result.accounts.map((a) => a.accountId)).toEqual([PRIMARY, SECONDARY]);
    expect(result.accounts[0]).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts[0]);
    expect(result.accounts[1]).toEqual({
      accountId: SECONDARY,
      alias: "Home",
      codexHome: "/b",
      monitored: false,
    });
  });

  it("keeps the first occurrence of a duplicated account id", () => {
    const result = sanitizeCodexAccountPreferences({
      accounts: [
        { accountId: PRIMARY, alias: "First", codexHome: "/1", monitored: true },
        { accountId: PRIMARY, alias: "Second", codexHome: "/2", monitored: false },
      ],
    });
    expect(result.accounts[0]).toEqual({
      accountId: PRIMARY,
      alias: "First",
      codexHome: "/1",
      monitored: true,
    });
    expect(result.accounts).toHaveLength(2);
  });

  it("fills a missing secondary account from the defaults in fixed order", () => {
    const result = sanitizeCodexAccountPreferences({
      accounts: [{ accountId: PRIMARY, alias: "Work", codexHome: "/a", monitored: false }],
    });
    expect(result.accounts).toEqual([
      { accountId: PRIMARY, alias: "Work", codexHome: "/a", monitored: false },
      { accountId: SECONDARY, alias: "", codexHome: "", monitored: true },
    ]);
  });

  it("replaces wrongly typed fields with per-account defaults", () => {
    const result = sanitizeCodexAccountPreferences({
      accounts: [
        { accountId: PRIMARY, alias: 42, codexHome: null, monitored: "yes" },
        { accountId: SECONDARY, alias: { x: 1 }, codexHome: 7, monitored: 0 },
      ],
    });
    expect(result.accounts).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts);
  });

  it("skips non-object entries without throwing", () => {
    const result = sanitizeCodexAccountPreferences({
      accounts: [null, "x", 3, [SECONDARY], { accountId: SECONDARY, alias: "S" }],
    });
    expect(result.accounts[1]).toEqual({
      accountId: SECONDARY,
      alias: "S",
      codexHome: "",
      monitored: true,
    });
  });

  it("looks up one account and returns null for an unknown id", () => {
    const accounts = DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts;
    expect(codexAccountSettings(accounts, SECONDARY)?.accountId).toBe(SECONDARY);
    expect(codexAccountSettings(accounts, "acme:corp")).toBeNull();
  });

  it("updates one account without touching the other", () => {
    const before = DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts;
    const after = withCodexAccount(before, SECONDARY, {
      alias: "Home",
      codexHome: "C:\\codex\\b",
      monitored: false,
    });
    expect(after).not.toBe(before);
    expect(after[0]).toEqual(before[0]);
    expect(after[1]).toEqual({
      accountId: SECONDARY,
      alias: "Home",
      codexHome: "C:\\codex\\b",
      monitored: false,
    });
    // The input is never mutated, so React state stays honest.
    expect(before[1]).toEqual({
      accountId: SECONDARY,
      alias: "",
      codexHome: "",
      monitored: true,
    });
  });

  it("patches the primary account alone when asked", () => {
    const after = withCodexAccount(DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts, PRIMARY, {
      monitored: false,
    });
    expect(after[0]).toEqual({ accountId: PRIMARY, alias: "", codexHome: "", monitored: false });
    expect(after[1]).toEqual(DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts[1]);
  });

  it("returns the input array itself for an unknown account id", () => {
    const before = DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts;
    const after = withCodexAccount(before, "acme:corp", { alias: "X" });
    expect(after).toBe(before);
    expect(after).toHaveLength(2);
  });

  it("treats the primary account as always configured", () => {
    const bare: CodexAccountSettings = {
      accountId: PRIMARY,
      alias: "",
      codexHome: "",
      monitored: true,
    };
    expect(isCodexAccountConfigured(bare)).toBe(true);
    expect(isCodexAccountConfigured({ ...bare, codexHome: "C:\\codex\\a" })).toBe(true);
  });

  it("requires an explicit home for the secondary account", () => {
    const bare: CodexAccountSettings = {
      accountId: SECONDARY,
      alias: "",
      codexHome: "",
      monitored: true,
    };
    expect(isCodexAccountConfigured(bare)).toBe(false);
    expect(isCodexAccountConfigured({ ...bare, codexHome: "   " })).toBe(false);
    expect(isCodexAccountConfigured({ ...bare, codexHome: "C:\\codex\\b" })).toBe(true);
  });
});
