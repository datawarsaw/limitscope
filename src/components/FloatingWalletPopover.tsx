import { useEffect, useId, useRef } from "react";
import type { CSSProperties, FocusEvent, ReactNode } from "react";
import { formatAge } from "../lib/format";
import type { FloatingWalletItem } from "../lib/floatingQuota";
import { floatingWalletBalanceText } from "../lib/floatingQuota";
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

/** The wallet's one-time state explanation, when the action needs one. */
function walletHint(item: FloatingWalletItem): string | null {
  if (item.state === "cooldown") {
    return "Cooling down: the last check hit OpenRouter's rate limit. Refresh resumes when it clears.";
  }
  if (item.errorHttpStatus === 429) {
    return "The last check was rate limited (HTTP 429).";
  }
  return null;
}

/**
 * The shared wallet detail card: the balance, its freshness, the credential
 * identity and one explicit Refresh. It is a dialog like the provider card
 * and uses the same hover/pin lifecycle; unlike a provider it has no quota
 * meter, windows or reset clock to render.
 */
export function FloatingWalletPopover({
  item,
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
  item: FloatingWalletItem;
  now: Date;
  pinned: boolean;
  align: number;
  exiting: boolean;
  busy: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
  onRefresh: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const hintId = useId();
  const cooling = item.state === "cooldown";
  const disabled = busy || cooling;
  const hint = walletHint(item);

  useEffect(() => {
    if (!pinned) return;
    const card = ref.current;
    if (!card) return;
    // Pinning lands focus on the card's action, like the provider card's
    // Open button. During a cooldown the action is disabled, so the dialog
    // itself becomes the focus target instead.
    const button = card.querySelector<HTMLButtonElement>(".fq-wallet-refresh");
    if (button && !button.disabled) {
      button.focus({ preventScroll: true });
    } else {
      card.focus({ preventScroll: true });
    }
  }, [pinned, item.id]);

  const balance = floatingWalletBalanceText(item);
  const freshness = freshnessLine(item, now);

  return (
    <div
      ref={ref}
      className={"fq-popover fq-wallet-popover" + (exiting ? " is-exiting" : "")}
      data-tauri-drag-region="false"
      data-wallet={item.id}
      data-state={item.state}
      role="dialog"
      aria-label={`${item.name} API wallet`}
      tabIndex={-1}
      style={{ "--fq-align": `${align}px` } as CSSProperties}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onFocus={onFocus}
      onBlur={onBlur}
    >
      <div className="fq-card" key={item.id}>
        <header className="fq-popover-head">
          <span className="fq-popover-mark" aria-hidden="true">
            <WalletMark />
          </span>
          <strong>{item.name}</strong>
          <span className={"fq-status status-" + walletStatusClass(item)}>
            {item.statusLabel}
          </span>
        </header>

        <p className="fq-value">
          <span
            className={"fq-value-num fq-wallet-balance" + (balance ? "" : " is-unavailable")}
          >
            {balance ?? "$ —"}
          </span>
          <span className="fq-value-unit">
            {balance ? "remaining" : "balance unavailable"}
          </span>
        </p>
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
          <p id={hintId} className="fq-wallet-hint">
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

        <div className="fq-actions fq-wallet-actions">
          <button
            type="button"
            className="fq-wallet-refresh"
            disabled={disabled}
            aria-busy={busy || undefined}
            aria-describedby={disabled && hint ? hintId : undefined}
            onClick={onRefresh}
          >
            {busy ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>
    </div>
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
