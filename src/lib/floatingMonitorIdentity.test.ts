// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  EMPTY_FLOATING_MONITOR,
  parseFloatingMonitorIdentity,
  sameFloatingMonitor,
} from "./floatingMonitorIdentity";
import {
  DEFAULT_FLOATING_PREFS,
  FLOATING_PREFS_STORAGE_KEY,
  loadFloatingQuotaPrefs,
  parseFloatingQuotaPrefs,
  saveFloatingQuotaPrefs,
} from "./floatingWindowPrefs";

function stubStorage() {
  const map = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
    clear: () => void map.clear(),
  });
  return map;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("floating monitor identity parsing", () => {
  it("reads a complete stored identity", () => {
    expect(
      parseFloatingMonitorIdentity({
        id: "\\.\DISPLAY2",
        x: 5160,
        y: 0,
        width: 1920,
        height: 1080,
      }),
    ).toEqual({
      id: "\\.\DISPLAY2",
      x: 5160,
      y: 0,
      width: 1920,
      height: 1080,
    });
  });

  it("reads an absent or corrupt identity as unknown", () => {
    expect(parseFloatingMonitorIdentity(null)).toEqual(EMPTY_FLOATING_MONITOR);
    expect(parseFloatingMonitorIdentity("nope")).toEqual(EMPTY_FLOATING_MONITOR);
    expect(parseFloatingMonitorIdentity([1, 2])).toEqual(EMPTY_FLOATING_MONITOR);
  });

  it("keeps the fields that are still usable when one is corrupt", () => {
    // A partially corrupt record degrades field by field, so a single bad
    // number does not throw away the device id that identifies the monitor.
    expect(
      parseFloatingMonitorIdentity({
        id: "\\.\DISPLAY2",
        x: "left",
        y: 10,
        width: Number.NaN,
        height: 20,
      }),
    ).toEqual({
      id: "\\.\DISPLAY2",
      x: null,
      y: 10,
      width: null,
      height: 20,
    });
  });

  it("rejects an empty device id", () => {
    expect(parseFloatingMonitorIdentity({ id: "" }).id).toBeNull();
  });
});

describe("sameFloatingMonitor", () => {
  it("compares by device id when both sides have one", () => {
    expect(
      sameFloatingMonitor(
        { id: "\\.\DISPLAY1", x: 0, y: 0, width: 1, height: 1 },
        { id: "\\.\DISPLAY1", x: 9, y: 9, width: 9, height: 9 },
      ),
    ).toBe(true);
    expect(
      sameFloatingMonitor(
        { id: "\\.\DISPLAY1", x: 0, y: 0, width: 1, height: 1 },
        { id: "\\.\DISPLAY2", x: 0, y: 0, width: 1, height: 1 },
      ),
    ).toBe(false);
  });

  it("compares by geometry when neither side has a device id", () => {
    expect(
      sameFloatingMonitor(
        { id: null, x: 0, y: 0, width: 1920, height: 1080 },
        { id: null, x: 0, y: 0, width: 1920, height: 1080 },
      ),
    ).toBe(true);
    expect(
      sameFloatingMonitor(
        { id: null, x: 0, y: 0, width: 1920, height: 1080 },
        { id: null, x: 1920, y: 0, width: 1920, height: 1080 },
      ),
    ).toBe(false);
  });

  it("never calls a named monitor the same as an unnamed one", () => {
    expect(
      sameFloatingMonitor(
        { id: "\\.\DISPLAY1", x: 0, y: 0, width: 1, height: 1 },
        { id: null, x: 0, y: 0, width: 1, height: 1 },
      ),
    ).toBe(false);
  });
});

describe("the monitor identity in the floating prefs", () => {
  it("defaults to an unknown monitor", () => {
    stubStorage();
    expect(loadFloatingQuotaPrefs().monitor).toEqual(EMPTY_FLOATING_MONITOR);
    expect(DEFAULT_FLOATING_PREFS.monitor).toEqual(EMPTY_FLOATING_MONITOR);
  });

  it("survives a prefs file written before the monitor field existed", () => {
    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: true,
        floatingBarEnabled: true,
        alwaysOnTop: true,
        dock: "none",
        dockAnchor: null,
        x: 200,
        y: 150,
      }),
    );
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.monitor).toEqual(EMPTY_FLOATING_MONITOR);
    expect(prefs.x).toBe(200);
    expect(prefs.y).toBe(150);
  });

  it("round-trips a stored identity", () => {
    const map = stubStorage();
    saveFloatingQuotaPrefs({
      ...loadFloatingQuotaPrefs(),
      monitor: {
        id: "\\.\DISPLAY3",
        x: -1440,
        y: 0,
        width: 1440,
        height: 2552,
      },
    });
    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.monitor.id).toBe("\\.\DISPLAY3");
    expect(loadFloatingQuotaPrefs().monitor.width).toBe(1440);
  });

  it("keeps the identity when an unrelated field is written", () => {
    stubStorage();
    saveFloatingQuotaPrefs({
      ...loadFloatingQuotaPrefs(),
      monitor: {
        id: "\\.\DISPLAY2",
        x: 5160,
        y: 0,
        width: 1920,
        height: 1080,
      },
    });
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), alwaysOnTop: false });
    expect(loadFloatingQuotaPrefs().monitor.id).toBe("\\.\DISPLAY2");
  });

  it("keeps the identity through a drag that saves a new position", () => {
    stubStorage();
    saveFloatingQuotaPrefs({
      ...loadFloatingQuotaPrefs(),
      monitor: {
        id: "\\.\DISPLAY2",
        x: 5160,
        y: 0,
        width: 1920,
        height: 1080,
      },
    });
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), x: 300, y: 220 });
    expect(loadFloatingQuotaPrefs().monitor.id).toBe("\\.\DISPLAY2");
    expect(loadFloatingQuotaPrefs().x).toBe(300);
  });

  it("resets the identity with the rest of the floating prefs", () => {
    stubStorage();
    saveFloatingQuotaPrefs({
      ...loadFloatingQuotaPrefs(),
      monitor: {
        id: "\\.\DISPLAY2",
        x: 5160,
        y: 0,
        width: 1920,
        height: 1080,
      },
    });
    const defaults = parseFloatingQuotaPrefs(null);
    expect(defaults.monitor).toEqual(EMPTY_FLOATING_MONITOR);
  });
});

describe("the floating prefs schema owns the monitor key", () => {
  it("does not let a foreign monitor key leak into the stored object", () => {
    const source = readFileSync(
      join(import.meta.dirname, "floatingWindowPrefs.ts"),
      "utf8",
    );
    expect(source).toContain('"monitor",');
  });
});

