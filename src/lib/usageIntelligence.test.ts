import { describe, expect, it } from "vitest";
import {
  cachedTokens,
  formatTokens,
  localMidnightMs,
  usageProviderLabel,
  usageSourceStateText,
} from "./usageIntelligence";

describe("usageIntelligence helpers", () => {
  it("formats compact token counts for fast comparison", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(9999)).toBe("9999");
    expect(formatTokens(10_000)).toBe("10K");
    expect(formatTokens(47_506)).toBe("47.5K");
    expect(formatTokens(1_040_000)).toBe("1M");
    expect(formatTokens(2_500_000_000)).toBe("2.5B");
    expect(formatTokens(Number.NaN)).toBe("—");
    expect(formatTokens(-1)).toBe("—");
  });

  it("combines cache read and cache write into the cached column", () => {
    expect(cachedTokens({ cacheReadTokens: 39_040, cacheWriteTokens: 512 })).toBe(39_552);
    expect(cachedTokens({ cacheReadTokens: 0, cacheWriteTokens: 0 })).toBe(0);
  });

  it("computes the local midnight boundary", () => {
    const midnight = localMidnightMs(new Date("2026-10-06T15:42:13"));
    const asDate = new Date(midnight);
    expect(asDate.getFullYear()).toBe(2026);
    expect(asDate.getMonth()).toBe(9);
    expect(asDate.getDate()).toBe(6);
    expect(asDate.getHours()).toBe(0);
    expect(asDate.getMinutes()).toBe(0);
    expect(asDate.getSeconds()).toBe(0);
  });

  it("labels the normalized provider axis", () => {
    expect(usageProviderLabel("zai")).toBe("Z.ai (ZCode)");
    expect(usageProviderLabel("openai-codex")).toBe("OpenAI / Codex");
    expect(usageProviderLabel("unknown")).toBe("Unknown provider");
    expect(usageProviderLabel("xai")).toBe("xai");
    expect(usageProviderLabel("unknown-provider")).toBe("unknown-provider");
  });

  it("words every diagnostic source state", () => {
    expect(usageSourceStateText("ok")).toBe("collecting");
    expect(usageSourceStateText("disabled")).toBe("off");
    expect(usageSourceStateText("sourceAbsent")).toBe("source not found");
    expect(usageSourceStateText("schemaUnsupported")).toBe("source schema not supported");
    expect(usageSourceStateText("readFailure")).toBe("could not read source");
    expect(usageSourceStateText("scanCeiling")).toBe("too many new rows to scan safely");
    expect(usageSourceStateText("collecting")).toBe("collecting now");
  });
});
