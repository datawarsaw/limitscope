// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";

const mocks = vi.hoisted(() => ({
  setLaunchAtStartup: vi.fn(),
  setRefreshInterval: vi.fn(),
  setTheme: vi.fn(),
  setQuotaNotifications: vi.fn(),
  setUsageIntelligence: vi.fn(),
  setProviderHidden: vi.fn(),
  setCodexAccount: vi.fn(),
  moveProvider: vi.fn(),
  setQuotaPerspective: vi.fn(),
  resetPreferences: vi.fn(async () => ({ ok: true, removed: true })),
  clearLocalHistory: vi.fn(async () => ({ ok: true, removed: true })),
  exportDiagnostics: vi.fn().mockResolvedValue({ status: "cancelled" }),
  exportUsageHistory: vi.fn().mockResolvedValue({
    status: "saved",
    fileName: "limitscope-usage-20260930T120000Z.csv",
  }),
  pickUsageExportDirectory: vi.fn().mockResolvedValue(null),
  refresh: vi.fn(),
}));

vi.mock("./lib/diagnosticsExport", () => ({
  exportDiagnostics: mocks.exportDiagnostics,
}));

vi.mock("./lib/usageExport", () => ({
  exportUsageHistory: mocks.exportUsageHistory,
  pickUsageExportDirectory: mocks.pickUsageExportDirectory,
}));

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
      usageIntelligence: false,
      codexAccounts: {
        accounts: [
          { accountId: "openai-codex", alias: "", codexHome: "", monitored: true },
          {
            accountId: "openai-codex:secondary",
            alias: "",
            codexHome: "",
            monitored: true,
          },
        ],
      },
    },
    setLaunchAtStartup: mocks.setLaunchAtStartup,
    setRefreshInterval: mocks.setRefreshInterval,
    setTheme: mocks.setTheme,
    setQuotaNotifications: mocks.setQuotaNotifications,
    setUsageIntelligence: mocks.setUsageIntelligence,
    setProviderHidden: mocks.setProviderHidden,
    moveProvider: mocks.moveProvider,
    setQuotaPerspective: mocks.setQuotaPerspective,
    setCodexAccount: mocks.setCodexAccount,
    resetPreferences: mocks.resetPreferences,
    startupPending: false,
    startupError: null,
  }),
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: [],
    loading: false,
    lastUpdatedAt: new Date("2026-09-28T21:58:00"),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
    usageIntelligenceRevision: 0,
  }),
}));

vi.mock("./hooks/useNow", () => ({
  useNow: () => new Date("2026-09-28T22:00:00").getTime(),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor: () => undefined,
    historyUnavailable: false,
    clearLocalHistory: mocks.clearLocalHistory,
  }),
}));

function settingsButton() {
  return screen.getByRole("button", { name: "Settings" });
}

function openSettings() {
  fireEvent.click(settingsButton());
}

describe("settings destination", () => {
  afterEach(() => {
    cleanup();
  });
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("keeps settings controls absent until the destination opens", () => {
    render(<App />);
    expect(
      screen.queryByRole("switch", {
        name: "Launch LimitScope when Windows starts",
      }),
    ).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Clear usage history" }),
    ).toBeNull();
  });

  it("shows the current update time and auto-refresh interval on both views", () => {
    render(<App />);
    expect(screen.getByText(/Updated/)).toBeTruthy();
    expect(screen.getByText("Auto-refresh · 15 min")).toBeTruthy();
    openSettings();
    expect(screen.getByText(/Updated/)).toBeTruthy();
  });

  it("opens Settings and exposes controls in three sections", () => {
    render(<App />);
    openSettings();
    expect(screen.getByRole("heading", { name: "General", level: 2 })).toBeTruthy();
    expect(
      screen.getByRole("switch", {
        name: "Launch LimitScope when Windows starts",
      }),
    ).toBeTruthy();
    expect(
      screen.getByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
    expect(screen.getByLabelText("Auto-refresh")).toBeTruthy();
    expect(screen.getByLabelText("Theme")).toBeTruthy();
    expect(screen.getByRole("radiogroup", { name: "Quota display" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    expect(
      screen.getByRole("switch", { name: "Usage Intelligence" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("group", { name: "Local data" }),
    ).toBeTruthy();
    expect(
      screen.getByText("Quota observations kept for 24 hours on this device."),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Clear usage history" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Clear provider cache" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Reset preferences" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Export diagnostics" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    // The wallet feature keeps a named seam inside its group.
    expect(
      screen.getByRole("group", { name: "API wallets" }),
    ).toBeTruthy();
  });

  it("keeps Codex account configuration connected after section navigation", () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Codex home of the Second account OpenAI / Codex account" }), { target: { value: "C:\\codex-secondary" } });
    expect(mocks.setCodexAccount).toHaveBeenCalledWith("openai-codex:secondary", { codexHome: "C:\\codex-secondary" });
  });

  it("preserves an unfinished confirmation when switching sections", () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear usage history" }));
    expect(screen.getByText("Clear usage history?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "General" }));
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    expect(screen.getByText("Clear usage history?")).toBeTruthy();
    expect(mocks.clearLocalHistory).not.toHaveBeenCalled();
  });

  it("returns to the Usage overview on the Usage button", () => {
    render(<App />);
    openSettings();
    expect(screen.getByLabelText("Theme")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    expect(screen.queryByLabelText("Theme")).toBeNull();
    expect(screen.getByLabelText("Usage overview")).toBeTruthy();
  });

  it("still fires the existing setting callbacks", () => {
    render(<App />);
    openSettings();
    fireEvent.click(
      screen.getByRole("switch", {
        name: "Launch LimitScope when Windows starts",
      }),
    );
    expect(mocks.setLaunchAtStartup).toHaveBeenCalledWith(true);
    fireEvent.click(screen.getByRole("switch", { name: "Quota notifications" }));
    expect(mocks.setQuotaNotifications).toHaveBeenCalledWith(true);
    fireEvent.change(screen.getByLabelText("Auto-refresh"), {
      target: { value: "5" },
    });
    expect(mocks.setRefreshInterval).toHaveBeenCalledWith(5);
    fireEvent.change(screen.getByLabelText("Theme"), {
      target: { value: "oled" },
    });
    expect(mocks.setTheme).toHaveBeenCalledWith("oled");
    fireEvent.click(screen.getByRole("radio", { name: "Remaining" }));
    expect(mocks.setQuotaPerspective).toHaveBeenCalledWith("remaining");
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("switch", { name: "Usage Intelligence" }));
    expect(mocks.setUsageIntelligence).toHaveBeenCalledWith(true);
  });

  it("confirms a clear before clearing the usage history", async () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Clear usage history" }),
    );
    expect(screen.getByText("Clear usage history?")).toBeTruthy();
    expect(mocks.clearLocalHistory).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: "Clear usage history" }),
    );
    await waitFor(() =>
      expect(mocks.clearLocalHistory).toHaveBeenCalledTimes(1),
    );
    expect(await screen.findByText("Usage history cleared.")).toBeTruthy();
  });

  it("clears the provider cache and resets preferences from the same section", async () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));

    fireEvent.click(
      screen.getByRole("button", { name: "Clear provider cache" }),
    );
    expect(screen.getByText("Clear provider cache?")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Clear provider cache" }),
    );
    expect(await screen.findByText("Provider cache cleared.")).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "Reset preferences" }),
    );
    expect(screen.getByText("Reset preferences?")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Reset preferences" }),
    );
    await waitFor(() => expect(mocks.resetPreferences).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Preferences reset.")).toBeTruthy();
  });

  it("exports diagnostics with success feedback", async () => {
    mocks.exportDiagnostics.mockResolvedValueOnce({
      status: "saved",
      fileName: "runtime-diagnostics-20260929T120000Z.json",
    });
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics" }));
    expect(mocks.exportDiagnostics).toHaveBeenCalledWith({
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
      usageIntelligence: false,
      codexAccounts: {
        accounts: [
          { accountId: "openai-codex", alias: "", codexHome: "", monitored: true },
          {
            accountId: "openai-codex:secondary",
            alias: "",
            codexHome: "",
            monitored: true,
          },
        ],
      },
    });
    expect(
      await screen.findByText(
        "Saved runtime-diagnostics-20260929T120000Z.json",
      ),
    ).toBeTruthy();
  });

  it("shows concise diagnostics failure feedback without raw errors", async () => {
    mocks.exportDiagnostics.mockRejectedValueOnce(
      new Error("Authorization: Bearer must-not-render"),
    );
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics" }));
    expect(await screen.findByText("Could not export diagnostics.")).toBeTruthy();
    expect(screen.queryByText(/must-not-render/)).toBeNull();
  });

  // WT-DIALOG remediation: the destination chooser replaced the native Save
  // As dialog, which could never settle on a refused destination (read-only
  // file) and left the export pending forever. Every step here settles.
  it("exports usage history as CSV through the destination chooser", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValueOnce("C:\\Exports");
    mocks.exportUsageHistory.mockResolvedValueOnce({
      status: "saved",
      fileName: "limitscope-usage-20260930T120000Z.csv",
    });
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    const save = screen.getByRole("button", { name: "Save" });
    expect(save.hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText("C:\\Exports");
    expect(save.hasAttribute("disabled")).toBe(false);
    fireEvent.click(save);
    expect(mocks.exportUsageHistory).toHaveBeenCalledWith("csv", {
      directory: "C:\\Exports",
      fileName: expect.stringMatching(/^limitscope-usage-\d{8}T\d{6}Z\.csv$/),
      confirmOverwrite: false,
    });
    expect(
      await screen.findByText("Saved limitscope-usage-20260930T120000Z.csv"),
    ).toBeTruthy();
    expect(
      screen
        .getByText("Saved limitscope-usage-20260930T120000Z.csv")
        .getAttribute("role"),
    ).toBe("status");
  });

  it("exports usage history as JSON with success feedback", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValueOnce("C:\\Exports");
    mocks.exportUsageHistory.mockResolvedValueOnce({
      status: "saved",
      fileName: "limitscope-usage-20260930T120000Z.json",
    });
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText("C:\\Exports");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText("Saved limitscope-usage-20260930T120000Z.json"),
    ).toBeTruthy();
  });

  it("reports a failed usage export through role=alert and clears busy", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValueOnce("C:\\Exports");
    mocks.exportUsageHistory.mockRejectedValueOnce(
      new Error("Access is denied must-not-render"),
    );
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText("C:\\Exports");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const alert = await screen.findByText("Could not export usage history.");
    expect(alert.getAttribute("role")).toBe("alert");
    expect(screen.queryByText(/must-not-render/)).toBeNull();
    // The busy state has cleared: the chooser is interactive again.
    expect(
      screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
    ).toBe(false);
  });

  it("asks for an explicit replace confirmation on an existing destination", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValue("C:\\Exports");
    mocks.exportUsageHistory
      .mockResolvedValueOnce({
        status: "confirm-overwrite",
        fileName: "existing.csv",
      })
      .mockResolvedValueOnce({ status: "saved", fileName: "existing.csv" });
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText("C:\\Exports");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(/already exists in the chosen folder/),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    expect(mocks.exportUsageHistory).toHaveBeenLastCalledWith(
      "csv",
      expect.objectContaining({ confirmOverwrite: true }),
    );
    expect(await screen.findByText("Saved existing.csv")).toBeTruthy();
  });

  it("declining the replace cancels without error or message", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValue("C:\\Exports");
    mocks.exportUsageHistory.mockResolvedValueOnce({
      status: "confirm-overwrite",
      fileName: "existing.csv",
    });
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText("C:\\Exports");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/already exists in the chosen folder/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mocks.exportUsageHistory).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/Saved /)).toBeNull();
    expect(screen.queryByText("Could not export usage history.")).toBeNull();
    expect(screen.getByRole("button", { name: "Export CSV" })).toBeTruthy();
  });

  it("closes the chooser without invoking when cancelled", async () => {
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mocks.exportUsageHistory).not.toHaveBeenCalled();
    expect(screen.queryByText(/Saved limitscope-usage/)).toBeNull();
    expect(screen.queryByText("Could not export usage history.")).toBeNull();
    expect(screen.getByRole("button", { name: "Export CSV" })).toBeTruthy();
  });

  it("keeps the chooser usable when the folder picker is cancelled", async () => {
    mocks.pickUsageExportDirectory.mockResolvedValueOnce(null);
    render(<App />);
    openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Choose folder…" })
          .hasAttribute("disabled"),
      ).toBe(false),
    );
    expect(screen.getByText("No folder chosen")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
    ).toBe(true);
    expect(mocks.exportUsageHistory).not.toHaveBeenCalled();
  });

  it("is keyboard reachable as a navigation destination", async () => {
    const user = userEvent.setup();
    render(<App />);
    const button = settingsButton();
    expect(button.tagName).toBe("BUTTON");
    button.focus();
    expect(document.activeElement).toBe(button);
    await user.keyboard("{Enter}");
    expect(screen.getByLabelText("Theme")).toBeTruthy();
    expect(
      button.getAttribute("aria-pressed"),
    ).toBe("true");
  });
});
