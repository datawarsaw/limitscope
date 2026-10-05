import type { CSSProperties } from "react";
import {
  dockMeterRects,
  dockSeparatorLefts,
  FLOATING_QUOTA_PERSPECTIVE,
  type FloatingQuotaItem,
} from "../lib/floatingQuota";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";

/**
 * The resting dock's meter strip: the five production meters, and nothing
 * else. Each meter is absolutely placed on the exact x position and width
 * the revealed Halo bar lays its own meter out at (computed by
 * dockMeterRects from the constants floating.css lays the real bar out
 * with), so the reveal crossfades each meter into itself — the strip reads
 * as a compressed continuation of the bar, not a separate component.
 *
 * No percentages, no labels, no provider marks, no glow. floating.css paints
 * the strip element itself as the faint resting surface (hairline boundary
 * across the full 600×16 footprint) so the hover target stays perceptible on
 * a dark desktop; the meters reuse
 * the bar's .fq-meter classes so the semantic remaining-quota scale, the
 * Halo fill gradient, and the 3px meter height come along unchanged; only
 * the placement (and with it the rounding) is dock-specific. Separators sit
 * on the revealed bar's segment boundaries. Like the bar's meters they stay
 * real progressbars for assistive tech, which is the only reading surface
 * while the pill itself is hidden.
 */
export function FloatingDockStrip({
  items,
  exiting,
}: {
  items: FloatingQuotaItem[];
  exiting: boolean;
}) {
  const rects = dockMeterRects(items.length);
  const separators = dockSeparatorLefts(items.length);
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
    </div>
  );
}
