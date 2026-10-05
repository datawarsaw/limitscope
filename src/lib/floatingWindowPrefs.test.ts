// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createTrailingPositionSave,
  DEFAULT_FLOATING_PREFS,
  FLOATING_PREFS_STORAGE_KEY,
  isFloatingPositionVisible,
  loadFloatingQuotaPrefs,
  parseFloatingQuotaPrefs,
  resetFloatingQuotaPrefs,
  saveFloatingQuotaPrefs,
} from "./floatingWindowPrefs";

const PRIMARY = { x: 0, y: 0, width: 1920, height: 1080 };
const SECOND = { x: 1920, y: 0, width: 1920, height: 1080 };

const FIXTURES_DIR = join(import.meta.dirname, "../../fixtures/settings-migration");

function readFixture(name: string): string {
  return readFileSync(join(FIXTURES_DIR, name), "utf8");
}

// The prefs API talks to localStorage, which is absent outside the browser;
// a Map-backed stub keeps the tests honest about what round-trips through
// JSON.
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

describe("floating prefs persistence", () => {
  it("defaults floatingBarEnabled to true for empty and corrupt storage", () => {
    stubStorage();
    expect(loadFloatingQuotaPrefs()).toEqual(DEFAULT_FLOATING_PREFS);
    expect(loadFloatingQuotaPrefs().floatingBarEnabled).toBe(true);

    stubStorage().set(FLOATING_PREFS_STORAGE_KEY, "{not json");
    expect(loadFloatingQuotaPrefs()).toEqual(DEFAULT_FLOATING_PREFS);
  });

  it("loads a stored disabled flag without dropping geometry", () => {
    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      readFixture("floating-prefs-disabled.json"),
    );
    expect(loadFloatingQuotaPrefs()).toEqual({
      visible: false,
      floatingBarEnabled: false,
      alwaysOnTop: false,
      dock: "none",
      dockAnchor: null,
      x: 200,
      y: 150,
    });
  });

  it("keeps the disabled flag when a drag saves a new position", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      readFixture("floating-prefs-disabled.json"),
    );
    const prefs = loadFloatingQuotaPrefs();
    saveFloatingQuotaPrefs({ ...prefs, x: 300, y: 220 });

    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.floatingBarEnabled).toBe(false);
    expect(stored.x).toBe(300);
    expect(stored.y).toBe(220);
    expect(stored.alwaysOnTop).toBe(false);
  });

  it("keeps geometry and pin across disable and re-enable", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: true,
        floatingBarEnabled: true,
        alwaysOnTop: true,
        x: 420,
        y: 96,
      }),
    );
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.x).toBe(420);
    expect(prefs.y).toBe(96);

    saveFloatingQuotaPrefs({ ...prefs, floatingBarEnabled: false });
    const disabled = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(disabled.floatingBarEnabled).toBe(false);
    expect(disabled.x).toBe(420);
    expect(disabled.y).toBe(96);
    expect(disabled.alwaysOnTop).toBe(true);

    const restored = loadFloatingQuotaPrefs();
    saveFloatingQuotaPrefs({ ...restored, floatingBarEnabled: true });
    const enabled = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(enabled.floatingBarEnabled).toBe(true);
    expect(enabled.x).toBe(420);
    expect(enabled.y).toBe(96);
    expect(enabled.alwaysOnTop).toBe(true);
  });

  it("treats hidden and disabled as independent states", () => {
    // Shape from the native acceptance harness (Flow 9/10): a hidden bar
    // is not a disabled one.
    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: false,
        floatingBarEnabled: true,
        alwaysOnTop: true,
        x: 200,
        y: 150,
      }),
    );
    const hidden = loadFloatingQuotaPrefs();
    expect(hidden.visible).toBe(false);
    expect(hidden.floatingBarEnabled).toBe(true);

    // A later hide or pin write must not flip the feature switch.
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), visible: false });
    saveFloatingQuotaPrefs({
      ...loadFloatingQuotaPrefs(),
      alwaysOnTop: false,
    });
    const after = loadFloatingQuotaPrefs();
    expect(after.floatingBarEnabled).toBe(true);
    expect(after.alwaysOnTop).toBe(false);
    expect(after.x).toBe(200);
  });

  it("passes foreign keys through and drops non-finite positions", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: true,
        alwaysOnTop: true,
        x: Number.NaN,
        y: "up",
        futureFloatingField: { autoPin: true },
      }),
    );
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.x).toBeNull();
    expect(prefs.y).toBeNull();

    saveFloatingQuotaPrefs({ ...prefs, x: 10, y: 20 });
    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.futureFloatingField).toEqual({ autoPin: true });
    expect(stored.x).toBe(10);
    expect(stored.y).toBe(20);
  });

  it("parse treats arrays and primitives as empty storage", () => {
    expect(parseFloatingQuotaPrefs(null)).toEqual(DEFAULT_FLOATING_PREFS);
    expect(parseFloatingQuotaPrefs([1, 2])).toEqual(DEFAULT_FLOATING_PREFS);
    expect(parseFloatingQuotaPrefs("nope")).toEqual(DEFAULT_FLOATING_PREFS);
    expect(parseFloatingQuotaPrefs({ visible: "yes" })).toEqual(
      DEFAULT_FLOATING_PREFS,
    );
  });
});

describe("floating dock pref persistence", () => {
  it("defaults the dock to none for empty, absent, and corrupt storage", () => {
    stubStorage();
    expect(loadFloatingQuotaPrefs().dock).toBe("none");

    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, alwaysOnTop: true, x: 1, y: 2 }),
    );
    expect(loadFloatingQuotaPrefs().dock).toBe("none");

    stubStorage().set(FLOATING_PREFS_STORAGE_KEY, "{not json");
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
  });

  it("loads a stored docked state without dropping geometry", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: true,
        floatingBarEnabled: true,
        alwaysOnTop: true,
        dock: "top",
        x: 1420,
        y: 0,
      }),
    );
    expect(loadFloatingQuotaPrefs().dock).toBe("top");

    // A later pin write keeps the dock state.
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), alwaysOnTop: false });
    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.dock).toBe("top");
    expect(stored.alwaysOnTop).toBe(false);
    expect(stored.x).toBe(1420);
  });

  it("reads any corrupted dock value as the free-floating bar", () => {
    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "bottom" }),
    );
    expect(loadFloatingQuotaPrefs().dock).toBe("none");

    stubStorage().set(FLOATING_PREFS_STORAGE_KEY, JSON.stringify({ dock: 1 }));
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
  });

  it("clears the dock on undock while keeping the stored position", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", x: 1420, y: 0 }),
    );
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), dock: "none" });
    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.dock).toBe("none");
    expect(stored.x).toBe(1420);
    expect(stored.y).toBe(0);
  });

  it("reset restores the undocked default", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top" }),
    );
    resetFloatingQuotaPrefs();
    expect(JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!).dock).toBe("none");
  });
});

describe("floating dockAnchor pref", () => {
  it("defaults to centered (null) for absent and corrupt values", () => {
    stubStorage();
    expect(loadFloatingQuotaPrefs().dockAnchor).toBeNull();

    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: "left" }),
    );
    expect(loadFloatingQuotaPrefs().dockAnchor).toBeNull();

    // Direct parse: non-finite numbers must also read as centered, though a
    // JSON round-trip would have already carried them as null.
    expect(
      parseFloatingQuotaPrefs({ dockAnchor: Number.NaN }).dockAnchor,
    ).toBeNull();
    expect(
      parseFloatingQuotaPrefs({ dockAnchor: Infinity }).dockAnchor,
    ).toBeNull();
  });

  it("clamps out-of-range finite anchors into the legal range", () => {
    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.8 }),
    );
    expect(loadFloatingQuotaPrefs().dockAnchor).toBe(0.5);

    stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: -3 }),
    );
    expect(loadFloatingQuotaPrefs().dockAnchor).toBe(-0.5);
  });

  it("loads and re-stores an off-center anchor with the dock state", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );
    expect(loadFloatingQuotaPrefs().dockAnchor).toBe(0.25);

    // An unrelated write keeps the anchor, like any other pref field.
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), alwaysOnTop: false });
    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.dockAnchor).toBe(0.25);
    expect(stored.dock).toBe("top");
    expect(stored.alwaysOnTop).toBe(false);
  });

  it("survives undock so re-docking returns to the user's spot, and reset clears it", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.4 }),
    );

    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), dock: "none" });
    expect(JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!).dockAnchor).toBe(0.4);

    resetFloatingQuotaPrefs();
    expect(JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!).dockAnchor).toBeNull();
  });
});

describe("floating prefs reset (v0.7 local data)", () => {
  it("restores the canonical defaults for visibility, switch, pin, and geometry", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: false,
        floatingBarEnabled: false,
        alwaysOnTop: false,
        x: 300,
        y: 200,
      }),
    );

    expect(resetFloatingQuotaPrefs()).toEqual(DEFAULT_FLOATING_PREFS);
    expect(JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!)).toEqual(
      DEFAULT_FLOATING_PREFS,
    );
    expect(loadFloatingQuotaPrefs()).toEqual(DEFAULT_FLOATING_PREFS);
  });

  it("preserves foreign keys instead of deleting them", () => {
    const map = stubStorage().set(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ x: 10, y: 20, futureFloatingField: { autoPin: true } }),
    );

    resetFloatingQuotaPrefs();

    const stored = JSON.parse(map.get(FLOATING_PREFS_STORAGE_KEY)!);
    expect(stored.x).toBeNull();
    expect(stored.y).toBeNull();
    expect(stored.futureFloatingField).toEqual({ autoPin: true });
  });

  it("is idempotent and safe on missing storage", () => {
    stubStorage();
    expect(() => {
      resetFloatingQuotaPrefs();
      resetFloatingQuotaPrefs();
    }).not.toThrow();
    expect(loadFloatingQuotaPrefs()).toEqual(DEFAULT_FLOATING_PREFS);
  });
});

describe("isFloatingPositionVisible", () => {
  it("accepts a position that sits fully on a work area", () => {
    expect(isFloatingPositionVisible(80, 48, 520, 64, [PRIMARY])).toBe(true);
  });

  it("accepts a window that straddles two monitors when either overlap is grabbable", () => {
    expect(isFloatingPositionVisible(1800, 40, 520, 64, [PRIMARY, SECOND])).toBe(true);
  });

  it("rejects a position left behind on a disconnected monitor", () => {
    expect(isFloatingPositionVisible(4000, 40, 520, 64, [PRIMARY])).toBe(false);
  });

  it("rejects a sliver that is not large enough to drag", () => {
    expect(isFloatingPositionVisible(-500, 40, 520, 64, [PRIMARY])).toBe(false);
  });
});

describe("createTrailingPositionSave", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("writes once, after the settle delay, with the last moved position", () => {
    const save = vi.fn();
    const saver = createTrailingPositionSave(200, save);
    saver.schedule(10, 20);
    vi.advanceTimersByTime(120);
    saver.schedule(30, 40);
    vi.advanceTimersByTime(199);
    expect(save).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(30, 40);
  });

  it("flush persists the pending position immediately and stops the timer", () => {
    const save = vi.fn();
    const saver = createTrailingPositionSave(200, save);
    saver.schedule(5, 6);
    saver.flush();
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(5, 6);
    vi.advanceTimersByTime(1000);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("cancel drops the pending position without writing", () => {
    const save = vi.fn();
    const saver = createTrailingPositionSave(200, save);
    saver.schedule(7, 8);
    saver.cancel();
    vi.advanceTimersByTime(1000);
    expect(save).not.toHaveBeenCalled();
  });

  it("flush and cancel are safe with nothing pending", () => {
    const save = vi.fn();
    const saver = createTrailingPositionSave(200, save);
    expect(() => {
      saver.flush();
      saver.cancel();
    }).not.toThrow();
    expect(save).not.toHaveBeenCalled();
  });
});
