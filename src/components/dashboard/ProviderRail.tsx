import { useCallback, useRef } from "react";
import {
  providerRailEntries,
  toneFor,
  type ProviderRailEntry,
} from "../../lib/dashboard";
import { providerStatusPresentation } from "../../lib/v03Integration";
import type { ProviderUsage } from "../../types";
import {
  mainQuotaPresentation,
  type QuotaPerspective,
} from "../../lib/quotaPresentation";

/**
 * Zone navigation: one compact entry per provider (mark, short name, primary
 * percent, tiny meter, status). Full provider detail lives in the main
 * column — the rail only navigates. It replaces the retired "every provider
 * is an equally large card" model and the quota strip's jump-to-card scroll:
 * selecting a rail entry swaps the primary panel and window list directly.
 *
 * Tabs semantics with a roving tabindex: the selected entry is the tab stop,
 * arrows/Home/End move (and select) without leaving the keyboard, and
 * aria-selected + the visual accent keep the active provider unmistakable.
 * Status is never color-alone: the accessible name carries the state word.
 */
export function ProviderRail({
  usages,
  selectedId,
  onSelect,
  orientation = "vertical",
  controlsId = "provider-panel",
  perspective,
}: {
  usages: readonly ProviderUsage[];
  selectedId: string | null;
  onSelect: (providerId: string) => void;
  /** Narrow windows lay the rail out horizontally; the tablist follows. */
  orientation?: "vertical" | "horizontal";
  /** id of the region the rail entries control (varies by view). */
  controlsId?: string;
  perspective: QuotaPerspective;
}) {
  const entries = providerRailEntries(usages);
  const listRef = useRef<HTMLDivElement>(null);

  const moveSelection = useCallback(
    (fromId: string | null, offset: 1 | -1 | "first" | "last") => {
      if (entries.length === 0) return;
      let index: number;
      if (offset === "first") index = 0;
      else if (offset === "last") index = entries.length - 1;
      else {
        const current = entries.findIndex((entry) => entry.usage.id === fromId);
        const base = current === -1 ? 0 : current;
        index = (base + offset + entries.length) % entries.length;
      }
      const next = entries[index];
      onSelect(next.usage.id);
      const button = listRef.current?.querySelector<HTMLButtonElement>(
        `#provider-tab-${CSS.escape(next.usage.id)}`,
      );
      button?.focus();
    },
    [entries, onSelect],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      const entryId = event.currentTarget.dataset.providerId ?? null;
      switch (event.key) {
        case "ArrowDown":
        case "ArrowRight":
          event.preventDefault();
          moveSelection(entryId, 1);
          break;
        case "ArrowUp":
        case "ArrowLeft":
          event.preventDefault();
          moveSelection(entryId, -1);
          break;
        case "Home":
          event.preventDefault();
          moveSelection(entryId, "first");
          break;
        case "End":
          event.preventDefault();
          moveSelection(entryId, "last");
          break;
        default:
          break;
      }
    },
    [moveSelection],
  );

  if (entries.length === 0) return null;

  return (
    <nav className="provider-rail" aria-label="Providers">
      <div
        className="provider-rail-list"
        role="tablist"
        aria-orientation={orientation}
        aria-label="Providers"
        ref={listRef}
      >
        {entries.map((entry, index) => (
          <RailItem
            key={entry.usage.id}
            entry={entry}
            selected={entry.usage.id === selectedId}
            // Nothing selected (Usage "All providers" scope): the first
            // entry stays a tab stop so the keyboard can always enter.
            tabStop={
              entry.usage.id === selectedId || (selectedId === null && index === 0)
            }
            onSelect={onSelect}
            onKeyDown={onKeyDown}
            controlsId={controlsId}
            perspective={perspective}
          />
        ))}
      </div>
    </nav>
  );
}

function RailItem({
  entry,
  selected,
  tabStop,
  onSelect,
  onKeyDown,
  controlsId,
  perspective,
}: {
  entry: ProviderRailEntry;
  selected: boolean;
  tabStop: boolean;
  onSelect: (providerId: string) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
  controlsId: string;
  perspective: QuotaPerspective;
}) {
  const { usage, shortName, mark, primary, percent } = entry;
  const status = providerStatusPresentation(usage);
  const presentation = mainQuotaPresentation(primary?.usedPercent, perspective);
  const tone = percent === undefined ? undefined : toneFor(percent);
  const meterWidth = Math.min(100, Math.max(0, presentation.meterPercent ?? 0));
  const state = status.label.toLowerCase();
  const spoken = primary
    ? `${usage.name}: ${presentation.label} in the ${primary.label.trim()} window, ${state}. Show provider details.`
    : `${usage.name}: ${state}. Show provider details.`;

  return (
    <button
      type="button"
      role="tab"
      id={`provider-tab-${usage.id}`}
      data-provider-id={usage.id}
      className={`rail-item${selected ? " selected" : ""}`}
      aria-selected={selected}
      aria-controls={controlsId}
      aria-label={spoken}
      tabIndex={tabStop ? 0 : -1}
      onClick={() => onSelect(usage.id)}
      onKeyDown={onKeyDown}
    >
      <span className={`rail-mark${selected ? " selected" : ""}`} aria-hidden="true">
        {mark}
      </span>
      <span className="rail-entry">
        <span className="rail-name-row">
          <span className="rail-name">{shortName}</span>
          <span className={`status-dot status-${status.className}`} aria-hidden="true" />
        </span>
        <span className="rail-meter" aria-hidden="true">
          <span
            className={`rail-meter-fill${tone ? ` tone-${tone}` : ""}`}
            style={{ width: `${meterWidth}%` }}
          />
        </span>
      </span>
      <span className="rail-percent" aria-hidden="true">
        {presentation.displayPercentText}
      </span>
    </button>
  );
}
