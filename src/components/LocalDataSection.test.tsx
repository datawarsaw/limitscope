// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LocalDataSection } from "./LocalDataSection";
import { localDataControls, type LocalDataClearResult } from "../lib/localData";

const OK: LocalDataClearResult = { ok: true, removed: true };

function renderSection(
  overrides: {
    usageHistory?: () => Promise<LocalDataClearResult>;
    providerCache?: () => Promise<LocalDataClearResult>;
    executionRuns?: () => Promise<LocalDataClearResult>;
    preferences?: () => Promise<LocalDataClearResult>;
    activeExecutionRun?: boolean;
  } = {},
) {
  const actions = {
    onClearUsageHistory: vi.fn(overrides.usageHistory ?? (async () => OK)),
    onClearProviderCache: vi.fn(overrides.providerCache ?? (async () => OK)),
    onClearExecutionRuns: vi.fn(overrides.executionRuns ?? (async () => OK)),
    onResetPreferences: vi.fn(overrides.preferences ?? (async () => OK)),
    activeExecutionRun: overrides.activeExecutionRun ?? false,
  };
  render(<LocalDataSection {...actions} />);
  return actions;
}

function control(id: string, activeExecutionRun = false) {
  const found = localDataControls(activeExecutionRun).find(
    (entry) => entry.id === id,
  );
  if (!found) throw new Error(`unknown control ${id}`);
  return found;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
});

describe("local data section", () => {
  it("renders the owned data classes as one compact group", () => {
    renderSection();
    expect(screen.getByRole("group", { name: "Local data" })).toBeTruthy();
    for (const entry of localDataControls(false)) {
      expect(
        screen.getByRole("button", {
          name: entry.confirmAction,
        }),
      ).toBeTruthy();
      expect(screen.getByText(entry.note)).toBeTruthy();
    }
  });

  it("asks for confirmation and states what is not affected", async () => {
    const user = userEvent.setup();
    const actions = renderSection();
    const history = control("usageHistory");

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );

    expect(screen.getByText(history.confirmTitle)).toBeTruthy();
    expect(screen.getByText(history.confirmBody)).toBeTruthy();
    expect(actions.onClearUsageHistory).not.toHaveBeenCalled();
  });

  it("names the destructive action in words, not color alone", async () => {
    const user = userEvent.setup();
    renderSection();
    const history = control("usageHistory");

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );

    expect(
      screen.getByRole("button", { name: history.confirmAction }).textContent,
    ).toBe(history.confirmAction);
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("holds focus in the confirmation and returns it to the control on cancel", async () => {
    const user = userEvent.setup();
    const actions = renderSection();
    const history = control("usageHistory");
    const trigger = screen.getByRole("button", {
      name: history.confirmAction,
    });

    await user.click(trigger);
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: history.confirmAction }),
    );

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText(history.confirmTitle)).toBeNull();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    expect(actions.onClearUsageHistory).not.toHaveBeenCalled();
  });

  it("clears after confirmation and shows a restrained success line", async () => {
    const user = userEvent.setup();
    const actions = renderSection();
    const history = control("usageHistory");

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    await user.click(screen.getByRole("button", { name: history.confirmAction }));

    expect(actions.onClearUsageHistory).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("status")).toHaveProperty(
      "textContent",
      history.success,
    );
    expect(screen.queryByText(history.confirmTitle)).toBeNull();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: history.confirmAction }),
    );
  });

  it("reports a scoped failure without claiming the data was removed", async () => {
    const user = userEvent.setup();
    const history = control("usageHistory");
    const actions = renderSection({ usageHistory: async () => ({ ok: false }) });

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    await user.click(screen.getByRole("button", { name: history.confirmAction }));

    expect(actions.onClearUsageHistory).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      history.failure,
    );
    expect(screen.queryByText(history.success)).toBeNull();
  });

  it("treats a rejected action as a scoped failure", async () => {
    const user = userEvent.setup();
    const history = control("usageHistory");
    renderSection({
      usageHistory: async () => {
        throw new Error("ipc unavailable");
      },
    });

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    await user.click(screen.getByRole("button", { name: history.confirmAction }));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      history.failure,
    );
  });

  it("drops a stale result line when a new confirmation opens", async () => {
    const user = userEvent.setup();
    const history = control("usageHistory");
    renderSection();

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    await user.click(screen.getByRole("button", { name: history.confirmAction }));
    expect(await screen.findByRole("status")).toBeTruthy();

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("routes each row to its own owned action", async () => {
    const user = userEvent.setup();
    const actions = renderSection();

    for (const [id, action] of [
      ["providerCache", actions.onClearProviderCache],
      ["preferences", actions.onResetPreferences],
    ] as const) {
      const entry = control(id);
      await user.click(
        screen.getByRole("button", { name: entry.confirmAction }),
      );
      await user.click(screen.getByRole("button", { name: entry.confirmAction }));
      expect(action).toHaveBeenCalledTimes(1);
      expect(await screen.findByText(entry.success)).toBeTruthy();
    }
    expect(actions.onClearUsageHistory).not.toHaveBeenCalled();
  });

  it("is keyboard operable end to end", async () => {
    const user = userEvent.setup();
    const actions = renderSection();
    const history = control("usageHistory");

    screen.getByRole("button", { name: history.confirmAction }).focus();
    await user.keyboard("{Enter}");
    expect(screen.getByText(history.confirmTitle)).toBeTruthy();
    await user.keyboard("{Enter}");

    expect(actions.onClearUsageHistory).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("status")).toBeTruthy();
  });

  it("disables the other rows while one clear is running", async () => {
    const user = userEvent.setup();
    let release: (value: LocalDataClearResult) => void = () => {};
    const pending = new Promise<LocalDataClearResult>((resolve) => {
      release = resolve;
    });
    const history = control("usageHistory");
    const cache = control("providerCache");
    renderSection({ usageHistory: () => pending });

    await user.click(
      screen.getByRole("button", { name: history.confirmAction }),
    );
    await user.click(screen.getByRole("button", { name: history.confirmAction }));

    expect(
      (screen.getByRole("button", {
        name: cache.confirmAction,
      }) as HTMLButtonElement).disabled,
    ).toBe(true);

    release(OK);
    expect(await screen.findByRole("status")).toBeTruthy();
    expect(
      (screen.getByRole("button", {
        name: cache.confirmAction,
      }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("confirms the active execution run by name and keeps it on cancel", async () => {
    const user = userEvent.setup();
    const actions = renderSection({ activeExecutionRun: true });
    const execution = control("executionRuns", true);

    await user.click(
      screen.getByRole("button", { name: execution.confirmAction }),
    );
    expect(screen.getByText(execution.confirmTitle)).toBeTruthy();
    expect(screen.getByText(execution.confirmBody)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(actions.onClearExecutionRuns).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: execution.confirmAction }),
    );
    await user.click(
      screen.getByRole("button", { name: execution.confirmAction }),
    );
    expect(actions.onClearExecutionRuns).toHaveBeenCalledTimes(1);
  });
});
