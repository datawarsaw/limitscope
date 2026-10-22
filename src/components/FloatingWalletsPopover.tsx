import { useEffect, useId, useRef } from "react";
import type { CSSProperties, FocusEvent, ReactNode } from "react";
import { formatAge } from "../lib/format";
import type { FloatingWalletItem } from "../lib/floatingQuota";
import { floatingWalletAmountText } from "../lib/floatingQuota";
import { WalletMark } from "./FloatingWalletSlot";

/** Status chip class for the wallet's own state vocabulary. */
function walletStatusClass(item: FloatingWalletItem): string {
  switch (item.state) {
    case "healthy":
      return "ok";
    case "stale":
    case "cooldown":
      return "stale";
    case "error":
      return "error";
    case "unavailable":
      return "unknown";
  }
}

/**
 * The observation line. A retained last-good balance keeps its original
 * timestamp, so it is described as last known rather than freshly checked.
 * No stamp never becomes "checked just now".
 */
function freshnessLine(
  item: FloatingWalletItem,
  now: Date,
): { muted: string; strong: string | null } {
  if (item.checkedAt) {
    return {
      muted: item.state === "healthy" ? "Checked " : "Last known ",
      strong: formatAge(item.checkedAt, now),
    };
  }
  if (item.state === "error") return { muted: "Last check failed", strong: null };
  if (item.state === "unavailable") {
    return { muted: "No balance recorded", strong: null };
  }
  return { muted: "Not checked yet", strong: null };
}

/**
 * The wallet's one-time state explanation, when the action needs one.
 *
 * The wording names the wallet it belongs to, so a card that lists several
 * wallets never attributes one wallet's rate limit to another.
 */
function walletHint(item: FloatingWalletItem): string | null {
  if (item.state === "cooldown") {
    return (
      "Cooling down: the last check hit " +
      item.name +
      "'s rate limit. Refresh resumes when it clears."
    );
  }
  if (item.errorHttpStatus === 429) {
    return "The last check was rate limited (HTTP 429).";
  }
  return null;
}

/**
 * A manual read can only help where the runtime did not settle a fresh
 * balance. A healthy wallet is maintained by the runtime's own polling, and
 * a cooling wallet has no supported read until its rate limit clears, so
 * neither gets an action that would either duplicate or fail.
 */
function walletNeedsManualRefresh(item: FloatingWalletItem): boolean {
  return (
    item.state === "stale" ||
    item.state === "error" ||
    item.state === "unavailable"
  );
}

/**
 * The shared API Wallets card: every prepaid wallet the runtime knows
 * about, one section each.
 *
 * It is a dialog like the provider card and uses the same hover/pin
 * lifecycle; unlike a provider it has no quota meter, windows or reset
 * clock to render. Each wallet states its own amounts, each with its own
 * currency: there is no combined total across currencies, because adding
 * USD to CNY would be meaningless.
 */
export function FloatingWalletsPopover({
  items,
  now,
  pinned,
  align,
  exiting,
  busy,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onBlur,
  onRefresh,
}: {
  items: FloatingWalletItem[];
  now: Date;
  pinned: boolean;
  align: number;
  exiting: boolean;
  busy: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
  /**
   * The runtime's manual wallet read. Absent means this surface cannot
   * acquire data, so no action is rendered at all.
   */
  onRefresh?: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const hintId = useId();
  const cooling = items.some((item) => item.state === "cooldown");
  const actionable = items.some(walletNeedsManualRefresh);
  // Hidden when every wallet is fresh or cooling; still shown while its own
  // read is in flight, so the in-flight state is not replaced by a vanish.
  const showRefresh = onRefresh !== undefined && (actionable || busy);
  const disabled = busy;

  useEffect(() => {
    if (!pinned) return;
    const card = ref.current;
    if (!card) return;
    // Pinning lands focus on the card's action when it has one; a card with
    // no actionable read (fresh or cooling wallets) focuses the dialog
    // itself, so the pin still has a keyboard target.
    const button = card.querySelector<HTMLButtonElement>(".fq-wallet-refresh");
    if (button && !button.disabled) {
      button.focus({ preventScroll: true });
    } else {
      card.focus({ preventScroll: true });
    }
  }, [pinned, items.length]);

  return (
    <div
      ref={ref}
      className={"fq-popover fq-wallet-popover" + (exiting ? " is-exiting" : "")}
      data-tauri-drag-region="false"
      data-wallet="api-wallets"
      role="dialog"
      aria-label="API wallets"
      tabIndex={-1}
      style={{ "--fq-align": align + "px" } as CSSProperties}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onFocus={onFocus}
      onBlur={onBlur}
    >
      <div className="fq-card" key="api-wallets">
        <header className="fq-popover-head">
          <span className="fq-popover-mark" aria-hidden="true">
            <WalletMark />
          </span>
          <strong>API Wallets</strong>
        </header>

        {items.map((item) => (
          <WalletSection key={item.id} item={item} now={now} hintId={hintId} />
        ))}

        {showRefresh ? (
          <div className="fq-actions fq-wallet-actions">
            <button
              type="button"
              className="fq-wallet-refresh"
              disabled={disabled}
              aria-busy={busy || undefined}
              aria-describedby={cooling ? hintId : undefined}
              onClick={onRefresh}
            >
              {busy ? "Refreshing…" : "Refresh"}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

/**
 * One wallet's section inside the shared card: its name and state, its
 * amounts (each with its currency), its freshness, and its masked
 * credential identity.
 */
function WalletSection({
  item,
  now,
  hintId,
}: {
  item: FloatingWalletItem;
  now: Date;
  hintId: string;
}) {
  const amounts = item.balances
    .map(floatingWalletAmountText)
    .filter((text): text is string => text !== null);
  const freshness = freshnessLine(item, now);
  const hint = walletHint(item);

  return (
    <section
      className="fq-wallet-section"
      data-wallet-id={item.id}
      data-state={item.state}
    >
      <header className="fq-wallet-section-head">
        <strong>{item.name}</strong>
        <span className={"fq-status status-" + walletStatusClass(item)}>
          {item.statusLabel}
        </span>
      </header>

      {amounts.length > 0 ? (
        <div className="fq-wallet-amounts">
          {amounts.map((amount) => (
            <p className="fq-value" key={amount}>
              <span className="fq-value-num fq-wallet-balance">{amount}</span>
              <span className="fq-value-unit">remaining</span>
            </p>
          ))}
        </div>
      ) : (
        <p className="fq-value">
          <span className="fq-value-num fq-wallet-balance is-unavailable">
            —
          </span>
          <span className="fq-value-unit">balance unavailable</span>
        </p>
      )}

      <p className="fq-primary-reset">
        {freshness.strong ? (
          <>
            {freshness.muted}
            <strong>{freshness.strong}</strong>
          </>
        ) : (
          freshness.muted
        )}
      </p>

      {item.error ? (
        <p className="fq-popover-error" role="alert">
          {item.error}
        </p>
      ) : null}
      {hint ? (
        <p className="fq-wallet-hint" id={hintId}>
          {hint}
        </p>
      ) : null}

      {item.identity ? (
        <footer className="fq-footer">
          <span className="fq-footer-icon" aria-hidden="true">
            <KeyIcon />
          </span>
          <p className="fq-popover-account">credential: {item.identity}</p>
        </footer>
      ) : null}
    </section>
  );
}

function KeyIcon(): ReactNode {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle
        cx="8"
        cy="8"
        r="3.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <path
        d="M10.5 10.5 20 20m-4-4-2 2m4.5-3.5-2 2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}
