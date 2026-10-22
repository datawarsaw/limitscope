import type { FloatingWalletItem } from "../lib/floatingQuota";
import {
  API_WALLETS_DETAIL_ID,
  floatingWalletSlotLabel,
  floatingWalletSlotState,
} from "../lib/floatingQuota";

/**
 * The wallet glyph. A wallet has identity but no quota window, so the mark
 * is the same kind of muted, single-color silhouette the provider segments
 * use — never a status color. The status dot below is the only state signal
 * in the slot; the money figures belong to the card, not the collapsed bar.
 */
export function WalletMark() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect
        x="3"
        y="5.5"
        width="18"
        height="13"
        rx="2.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <path
        fill="currentColor"
        d="M14.25 10.5H21v3h-6.75a1.5 1.5 0 0 1 0-3Z"
      />
    </svg>
  );
}

/**
 * The compact API wallet slot (Variant B): a fixed 44px segment after the
 * providers. It opens the same shared detail card as a provider segment and
 * carries no text beyond its mark and status dot — assistive tech gets the
 * balances and states from the spoken label.
 *
 * There is exactly one wallet slot, whatever the number of wallets the
 * runtime reports: it is the shared entry point into the API Wallets card,
 * never one segment per wallet. The slot's dot states the most
 * consequential state among the wallets it covers.
 */
export function FloatingWalletSlot({
  items,
  expanded,
  busy,
  onHover,
  onHoverEnd,
  onOpen,
  onContextMenu,
}: {
  items: FloatingWalletItem[];
  expanded: boolean;
  busy: boolean;
  onHover: (id: string) => void;
  onHoverEnd: () => void;
  onOpen: (id: string) => void;
  onContextMenu: () => void;
}) {
  const state = floatingWalletSlotState(items) ?? "unavailable";
  return (
    <button
      type="button"
      className={
        "fq-wallet is-" + state + (busy ? " is-refreshing" : "")
      }
      data-wallet="api-wallets"
      data-wallet-id={API_WALLETS_DETAIL_ID}
      data-state={state}
      aria-label={floatingWalletSlotLabel(items)}
      aria-haspopup="dialog"
      aria-expanded={expanded}
      aria-busy={busy || undefined}
      onMouseEnter={() => onHover(API_WALLETS_DETAIL_ID)}
      onMouseLeave={onHoverEnd}
      onFocus={() => onHover(API_WALLETS_DETAIL_ID)}
      onBlur={onHoverEnd}
      onClick={() => onOpen(API_WALLETS_DETAIL_ID)}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onContextMenu();
      }}
    >
      <span className="fq-wallet-mark">
        <WalletMark />
      </span>
      <span className="fq-wallet-dot" aria-hidden="true" />
    </button>
  );
}
