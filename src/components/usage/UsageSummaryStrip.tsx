import type { ReactNode } from "react";
import {
  constrainedValuePresentation,
  counterpartPercentText,
  exactnessCaption,
  exactnessCaptionFor,
  formatObservedAt,
  nearLimitEquivalenceNote,
  nearLimitPresentation,
  peakPresentation,
  severityWord,
} from "../../lib/usagePresentation";
import { shortProviderId } from "../../lib/dashboard";
import type { QuotaPerspective } from "../../lib/quotaPresentation";
import type { ObservedPeak, UsageAnalytics } from "../../lib/usageAnalytics";

/**
 * The five research-proven summary metrics, straight from the backend
 * summary — never recomputed from trend arrays. Every non-exact or estimated
 * value carries its qualifier as text ("lower bound", "est.") so the
 * semantics survive color blindness, themes, and screen readers.
 *
 * The quota-perspective preference shapes the value text only ("84% used" /
 * "16% remaining"); severity stays canonical on the used percentage, so
 * 95% used reads Critical next to "5% remaining".
 */

function identityText(peak: ObservedPeak): string {
  const account = peak.account ? ` (${peak.account})` : "";
  return `${shortProviderId(peak.providerId)}${account} · ${peak.windowLabel}`;
}

function Metric({
  label,
  value,
  badge,
  badgeTone,
  sub,
  subLines,
}: {
  label: string;
  value: string;
  badge?: string;
  badgeTone?: "neutral" | "warn" | "critical";
  /** Single faint detail line. */
  sub?: string;
  /** Detail lines rendered separately (breakdowns stay individually readable
   * and individually addressable by tests and screen readers). */
  subLines?: ReactNode[];
}) {
  return (
    <div className="usage-metric">
      <span className="usage-metric-label">{label}</span>
      <span className="usage-metric-value-row">
        <span className="usage-metric-value">{value}</span>
        {badge ? (
          <span className={`usage-badge usage-badge-${badgeTone ?? "neutral"}`}>
            {badge}
          </span>
        ) : null}
      </span>
      {sub !== undefined ? (
        <span className="usage-metric-sub">{sub}</span>
      ) : null}
      {(subLines ?? []).map((line, index) => (
        <span key={index} className="usage-metric-sub">
          {line}
        </span>
      ))}
    </div>
  );
}

export function UsageSummaryStrip({
  analytics,
  perspective = "used",
}: {
  analytics: UsageAnalytics;
  perspective?: QuotaPerspective;
}) {
  const { summary } = analytics;
  const peak = summary.peakObservedUsage;
  const constrained = summary.mostConstrainedWindow;
  const nearLimit = nearLimitPresentation(summary.timeNearLimit);
  const peakShown = peak ? peakPresentation(peak, perspective) : undefined;
  const constrainedExactness = constrained
    ? exactnessCaptionFor(constrained.exactness, perspective)
    : undefined;
  // Canonical: severity is derived from the used percentage in both modes.
  const severity = constrained ? severityWord(constrained.usedPercent) : undefined;

  const constrainedSubLines: ReactNode[] = constrained
    ? [
        identityText(constrained),
        counterpartPercentText(constrained.usedPercent, perspective),
        ...(constrainedExactness ? [constrainedExactness] : []),
      ]
    : [];

  return (
    <dl className="usage-summary" aria-label="Usage summary">
      <Metric
        label="Peak observed"
        value={peakShown ? peakShown.value : "—"}
        badge={peakShown?.badge}
        sub={
          peak
            ? `${identityText(peak)} · ${formatObservedAt(peak.observedAt)}`
            : "no observations in range"
        }
      />
      <Metric
        label="Most constrained"
        value={
          constrained
            ? constrainedValuePresentation(constrained.usedPercent, perspective)
            : "—"
        }
        badge={severity ?? constrainedExactness}
        badgeTone={
          severity === "Critical"
            ? "critical"
            : severity === "High"
              ? "warn"
              : "neutral"
        }
        sub={constrained ? undefined : "no observations in range"}
        subLines={constrainedSubLines}
      />
      <Metric
        label="Observed resets"
        value={String(summary.observedResetCycles.count)}
        badge={exactnessCaption(summary.observedResetCycles.exactness)}
        sub="detected transitions"
      />
      <Metric
        label="Observed days"
        value={String(summary.observedDays.count)}
        sub={`of the last ${analytics.heatmap.length} days`}
      />
      <Metric
        label="Near limit"
        value={nearLimit ? `~${nearLimit.primary.text}` : "—"}
        badge={nearLimit ? "est." : undefined}
        sub={
          nearLimit
            ? undefined
            : (nearLimitEquivalenceNote(perspective) ?? "not enough comparable data")
        }
        subLines={
          nearLimit
            ? [
                `≥80% used · ${nearLimit.primary.text}`,
                ...(nearLimit.secondary
                  ? [`≥95% used · ${nearLimit.secondary.text}`]
                  : []),
                ...(nearLimitEquivalenceNote(perspective)
                  ? [nearLimitEquivalenceNote(perspective)!]
                  : []),
              ]
            : nearLimitEquivalenceNote(perspective)
              ? [nearLimitEquivalenceNote(perspective)!]
              : []
        }
      />
    </dl>
  );
}

/**
 * Estimated total used by the strip: kept next to the presentation so tests
 * can pin the "est." wording against the backend's own numbers.
 */
export function nearLimitHeadline(
  analytics: UsageAnalytics,
): string | undefined {
  const nearLimit = nearLimitPresentation(analytics.summary.timeNearLimit);
  return nearLimit ? `~${nearLimit.primary.text}` : undefined;
}
