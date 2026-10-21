import type { FloatingWalletItem } from "../lib/floatingQuota";
import { floatingWalletLabel } from "../lib/floatingQuota";

/**
 * The wallet glyph. A wallet has identity but no quota window, so the mark
 * is the same kind of muted, single-color silhouette the provider segments
 * use — never a status color. The status dot below is the only state signal
 * in the slot; the dollar figure belongs to the card, not the collapsed bar.
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
 * balance and state from the spoken label.
 */
export function FloatingWalletSlot({
  item,
  expanded,
  busy,
  onHover,
  onHoverEnd,
  onOpen,
  onContextMenu,
}: {
  item: FloatingWalletItem;
  expanded: boolean;
  busy: boolean;
  onHover: (id: string) => void;
  onHoverEnd: () => void;
  onOpen: (id: string) => void;
  onContextMenu: () => void;
}) {
  return (
    <button
      type="button"
      className={
        "fq-wallet is-" +
        item.state +
        (busy ? " is-refreshing" : "")
      }
      data-wallet={item.id}
      data-wallet-id={item.id}
      data-state={item.state}
      aria-label={floatingWalletLabel(item)}
      aria-haspopup="dialog"
      aria-expanded={expanded}
      aria-busy={busy || undefined}
      onMouseEnter={() => onHover(item.id)}
      onMouseLeave={onHoverEnd}
      onFocus={() => onHover(item.id)}
      onBlur={onHoverEnd}
      onClick={() => onOpen(item.id)}
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
