import { describe, expect, it } from "vitest";
import {
  CODEX_ACCOUNT_IDS,
  CODEX_PROVIDER_ID,
  CODEX_SECONDARY_PROVIDER_ID,
  baseProviderId,
  isCodexAccountScopedId,
  isCodexProviderId,
} from "./providerIds";

describe("provider ids", () => {
  it("exposes the two configured Codex account ids in order", () => {
    expect(CODEX_PROVIDER_ID).toBe("openai-codex");
    expect(CODEX_SECONDARY_PROVIDER_ID).toBe("openai-codex:secondary");
    expect(CODEX_ACCOUNT_IDS).toEqual(["openai-codex", "openai-codex:secondary"]);
  });

  it("resolves an account-scoped id to its base provider", () => {
    expect(baseProviderId("openai-codex")).toBe("openai-codex");
    expect(baseProviderId("openai-codex:secondary")).toBe("openai-codex");
    expect(baseProviderId("zai")).toBe("zai");
    expect(baseProviderId("")).toBe("");
  });

  it("splits on the first colon only", () => {
    expect(baseProviderId("acme:corp:extra")).toBe("acme");
  });

  it("recognises every Codex id as the Codex provider", () => {
    expect(isCodexProviderId("openai-codex")).toBe(true);
    expect(isCodexProviderId("openai-codex:secondary")).toBe(true);
    expect(isCodexProviderId("zai")).toBe(false);
    expect(isCodexProviderId("")).toBe(false);
  });

  it("marks only the secondary Codex id as account-scoped", () => {
    expect(isCodexAccountScopedId("openai-codex:secondary")).toBe(true);
    expect(isCodexAccountScopedId("openai-codex")).toBe(false);
    expect(isCodexAccountScopedId("acme:corp")).toBe(false);
    expect(isCodexAccountScopedId("")).toBe(false);
  });
});
