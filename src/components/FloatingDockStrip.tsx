import type { CSSProperties } from "react";
import {
  dockMeterRects,
  dockSeparatorLefts,
  dockWalletDotRect,
  FLOATING_QUOTA_PERSPECTIVE,
  floatingWalletSlotLabel,
  floatingWalletSlotState,
  type FloatingWalletItem,
  type FloatingQuotaItem,
} from "../lib/floatingQuota";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";

/**
 * The resting dock's meter strip: the production meters, and nothing else.
 * Each meter is absolutely placed on the exact x position and width the
 * revealed Halo bar lays its own meter out at (computed by dockMeterRects
 * from the constants floating.css lays the real bar out with), so the
 * reveal crossfades each meter into itself — the strip reads as a
 * compressed continuation of the bar, not a separate component.
 *
 * The strip's width is the window's own width — the revealed bar's width for
 * the same visible providers — so the dock never leaves empty space on the
 * desktop for providers the user hid.
 *
 * No percentages, no labels, no provider marks, no glow. floating.css paints
 * the strip element itself as the faint resting surface (hairline boundary
 * across the footprint) so the hover target stays perceptible on a dark
 * desktop; the meters reuse the bar's .fq-meter classes so the semantic
 * remaining-quota scale, the Halo fill gradient, and the 3px meter height
 * come along unchanged; only the placement (and with it the rounding) is
 * dock-specific. Separators sit on the revealed bar's segment boundaries.
 * Like the bar's meters they stay real progressbars for assistive tech, which
 * is the only reading surface while the pill itself is hidden.
 *
 * A wallet adds one status DOT in the revealed bar's own wallet slot. A
 * wallet is an absolute money balance, not a quota window, so it never gets
 * a meter, a fill, or a percentage here — only presence and health.
 */
export function FloatingDockStrip({
  items,
  wallets = [],
  exiting,
}: {
  items: FloatingQuotaItem[];
  /** The revealed bar carries a wallet slot after the providers. */
  wallets?: FloatingWalletItem[];
  exiting: boolean;
}) {
  const hasWallet = wallets.length > 0;
  const rects = dockMeterRects(items.length, hasWallet);
  const separators = dockSeparatorLefts(items.length, hasWallet);
  const walletDot = dockWalletDotRect(items.length, hasWallet);
  // The bar carries ONE shared wallet slot, whatever the number of wallets
  // the runtime reports. The resting dot is that same slot, so it states the
  // most consequential state among the wallets it covers and speaks the same
  // label as the revealed slot — the two never disagree.
  const walletState = floatingWalletSlotState(wallets);
  return (
    <div
      className={"fq-dock-strip" + (exiting ? " is-exiting" : "")}
      data-tauri-drag-region="deep"
      aria-label="Provider quotas"
      role="group"
    >
      {separators.map((left) => (
        <span
          key={left}
          className="fq-dock-sep"
          aria-hidden="true"
          style={{ left } as CSSProperties}
        />
      ))}
      {items.map((item, index) => {
        const presentation = floatingQuotaPresentation(
          item.usedPercent ?? item.percent,
          FLOATING_QUOTA_PERSPECTIVE,
        );
        const rect = rects[index];
        return (
          <span
            key={item.providerId}
            className="fq-meter fq-dock-meter"
            data-quota={quotaColorLevel(presentation)}
            role="progressbar"
            aria-label={`${item.name} quota · ${presentation.ariaLabel}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={presentation.meterAriaValueNow ?? undefined}
            aria-valuetext={presentation.meterAriaValueText}
            style={{ left: rect.left, width: rect.width } as CSSProperties}
          >
            <span
              className="fq-meter-fill"
              style={{ width: `${presentation.meterPercent ?? 0}%` }}
            />
          </span>
        );
      })}
      {walletDot && walletState ? (
        <span
          className="fq-dock-wallet-dot"
          data-state={walletState}
          // The same spoken contract the revealed wallet slot uses: every
          // wallet's amount, each in its own currency, plus the status. The
          // balance is named as a balance — never a quota percentage — and
          // the dot itself carries no meter fill.
          role="img"
          aria-label={floatingWalletSlotLabel(wallets)}
          style={{ left: walletDot.left } as CSSProperties}
        />
      ) : null}
    </div>
  );
}
