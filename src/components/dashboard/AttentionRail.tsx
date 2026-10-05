import type { AttentionItem, AttentionKind } from "../../lib/dashboard";

/** Dot classes reuse the shared status colors so OLED's no-glow rule and the
 * theme's severity palette apply unchanged. The item's words — not the dot —
 * carry the meaning, so state is never color-alone. */
const KIND_DOT: Record<AttentionKind, string> = {
  critical: "status-error",
  error: "status-error",
  stale: "status-stale",
  exhaustion: "status-stale",
  warn: "status-stale",
  unknown: "status-unknown",
};

/**
 * Zone C — a narrow rail of things that deserve a glance, derived entirely
 * from existing runtime state (see `attentionItems`): quota thresholds,
 * stale sources, failed refreshes, unknown sources, and engine-confirmed
 * exhaustion risk. It is not a notification center: no history, no actions
 * beyond focusing the provider. Selecting an item just anchors Zone A/B on
 * that provider. With nothing to say it stays calm: one quiet line, no
 * celebratory visuals.
 */
export function AttentionRail({
  items,
  onSelect,
}: {
  items: readonly AttentionItem[];
  onSelect: (providerId: string) => void;
}) {
  return (
    <aside className="attention-panel" aria-label="Needs attention">
      <h3 className="panel-heading">Needs attention</h3>
      {items.length === 0 ? (
        <p className="attention-empty">
          <svg
            className="attention-check"
            viewBox="0 0 12 12"
            fill="none"
            aria-hidden="true"
          >
            <path
              d="M2.5 6.5l2.4 2.4L9.5 3.8"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
          All providers look healthy
        </p>
      ) : (
        <ul className="attention-list">
          {items.map((item) => (
            <li key={item.providerId}>
              <button
                type="button"
                className={`attention-item kind-${item.kind}`}
                onClick={() => onSelect(item.providerId)}
                aria-label={`${item.providerName}: ${item.line}. Show provider details.`}
              >
                <span
                  className={`status-dot ${KIND_DOT[item.kind]}`}
                  aria-hidden="true"
                />
                <span className="attention-copy">
                  <span className="attention-name">{item.providerName}</span>
                  <span className="attention-line">{item.line}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
