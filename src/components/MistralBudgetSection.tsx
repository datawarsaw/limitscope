import type { MistralBudgetRow, MistralBudgets } from "../types";
import { FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";

/**
 * The Mistral budget detail: the Vibe budget and the API budget, each with
 * its own percentage, its own reset and its own money, plus the verified
 * plan name.
 *
 * The two rows are independent by construction. A row the source did not
 * report (or reported unusably) states that it is unavailable and nothing
 * else — never zero, never a full budget, and never a number borrowed from
 * its sibling. The two budgets are never summed: they are two separate
 * allowances of the same account.
 */

/** Fixed short month names: the reset readout stays the same everywhere. */
const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

/**
 * One money amount in its own currency, always with two decimals.
 *
 * The currency travels with the amount: EUR cents are never published as a
 * bare number a consumer could read as dollars. Null means there is no
 * trustworthy amount to state — the surface never claims a zero.
 */
export function mistralMoney(
  currency: string | undefined,
  cents: number | undefined,
): string | null {
  if (typeof cents !== "number" || !Number.isFinite(cents)) return null;
  const code = currency?.trim();
  if (!code) return null;
  return `${code} ${(cents / 100).toFixed(2)}`;
}

/**
 * The reset readout of one budget row: the local-time instant of the
 * source's own reset stamp, as "1 Dec 01:00".
 *
 * The stamp itself is preserved verbatim by the runtime; this is only its
 * local reading. An unreadable stamp has no readout at all.
 */
export function mistralResetText(iso: string | undefined): string | null {
  if (!iso) return null;
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return null;
  const hours = String(target.getHours()).padStart(2, "0");
  const minutes = String(target.getMinutes()).padStart(2, "0");
  return `${target.getDate()} ${MONTHS[target.getMonth()]} ${hours}:${minutes}`;
}

/**
 * Why one budget row is unknown, in the runtime's fixed vocabulary. The
 * reasons name the missing fact, so the card never shows a bare "—" where
 * it could say what was not reported.
 */
export function mistralUnavailableReason(reason: string | undefined): string {
  switch (reason) {
    case "missing":
      return "Not reported by the source";
    case "currency":
      return "Currency not reported";
    case "initial_budget":
      return "Budget total not reported";
    case "usage_percentage":
      return "Usage not reported";
    case "reset_at":
      return "Reset time not readable";
    default:
      return "Unavailable";
  }
}

function MistralBudgetRowView({ row }: { row: MistralBudgetRow }) {
  if (!row.available) {
    return (
      <div className="fq-mistral-row is-unavailable" data-mistral-row={row.key}>
        <span className="fq-mistral-label">{row.label}</span>
        <span className="fq-mistral-state">Unavailable</span>
        <p className="fq-mistral-reason">
          {mistralUnavailableReason(row.unavailableReason)}
        </p>
      </div>
    );
  }
  const presentation = floatingQuotaPresentation(
    row.usedPercent,
    FLOATING_QUOTA_PERSPECTIVE,
  );
  const used = mistralMoney(row.currency, row.usedCents);
  const total = mistralMoney(row.currency, row.totalCents);
  const remaining = mistralMoney(row.currency, row.remainingCents);
  const reset = mistralResetText(row.resetAt);
  return (
    <div className="fq-mistral-row" data-mistral-row={row.key}>
      <span className="fq-mistral-label">{row.label}</span>
      <span className="fq-mistral-state">
        {presentation.displayPercentText}
        {presentation.displayPercent !== null ? " remaining" : ""}
      </span>
      <p
        className="fq-mistral-amounts"
        title={
          row.amountDerived
            ? "The used amount is derived from the reported percentage and budget total, rounded to the cent."
            : undefined
        }
      >
        {used ? <span>{used} used</span> : null}
        {total ? <span>{total} total</span> : null}
        {remaining ? <span>{remaining} remaining</span> : null}
      </p>
      <p className="fq-mistral-reset">
        {reset ? `Resets ${reset}` : "No reset time"}
      </p>
      <span
        className="fq-meter fq-meter-row"
        data-quota={quotaColorLevel(presentation)}
        role="progressbar"
        aria-label={`${row.label} quota · ${presentation.ariaLabel}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={presentation.meterAriaValueNow ?? undefined}
        aria-valuetext={presentation.meterAriaValueText}
      >
        <span
          className="fq-meter-fill"
          style={{ width: `${presentation.meterPercent ?? 0}%` }}
        />
      </span>
    </div>
  );
}

/**
 * The Mistral detail block: both monthly budgets and the verified plan.
 *
 * The plan chip appears only when the subscription endpoint verified it, so
 * an unverified plan is simply absent.
 */
export function MistralBudgetSection({ budgets }: { budgets: MistralBudgets }) {
  return (
    <section className="fq-mistral" aria-label="Mistral monthly budgets">
      <div className="fq-mistral-head">
        <strong>Monthly budgets</strong>
        {budgets.planName ? (
          <span className="fq-mistral-plan">{budgets.planName} plan</span>
        ) : null}
      </div>
      {budgets.rows.map((row) => (
        <MistralBudgetRowView key={row.key} row={row} />
      ))}
    </section>
  );
}
