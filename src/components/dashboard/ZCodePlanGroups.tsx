import type { ProviderUsage } from "../../types";
import { formatResetTime } from "../../lib/format";
import {
  balanceLabel,
  formatBalanceValue,
  planGroupsFor,
} from "../../lib/zcodePlans";
import type { ZCodePlan, ZCodePlanBalance } from "../../types";

/**
 * Zone B supplement — the active ZCode plans/packages beyond the coding
 * plan, grouped under the Z.ai card exactly as upstream reports them: one
 * visually separated group per plan, its absolute balances as rows under
 * it. The regular quota windows above stay untouched; these rows never
 * feed the percent meters, the strip, or predictions (they are not
 * LimitWindow data), and rows never merge across units or plans.
 *
 * Expiry language: a repeating period resets ("Resets …", with the reset
 * readout the windows use), a one-time package expires ("Expires …").
 * Renders nothing for providers without an observation.
 */
export function ZCodePlanGroups({
  usage,
  now,
}: {
  usage: ProviderUsage;
  now: Date;
}) {
  const plans = planGroupsFor(usage);
  if (!plans || plans.length === 0) return null;
  return (
    <div className="zcode-plans">
      {plans.map((plan) => (
        <PlanGroup key={plan.planId} plan={plan} now={now} />
      ))}
    </div>
  );
}

function PlanGroup({ plan, now }: { plan: ZCodePlan; now: Date }) {
  const label = plan.name ?? "ZCode plan";
  return (
    <div className="zcode-plan" role="group" aria-label={`ZCode plan ${label}`}>
      <div className="zcode-plan-head">
        <span className="zcode-plan-name">{label}</span>
        {plan.endsAt ? (
          <span className="zcode-plan-expiry">
            Expires {formatResetTime(plan.endsAt, now)}
          </span>
        ) : null}
      </div>
      {plan.balances.map((balance, index) => (
        <BalanceRow
          key={balance.bucketId ?? `${balance.entitlementId ?? "bucket"}-${index}`}
          balance={balance}
          now={now}
        />
      ))}
    </div>
  );
}

function BalanceRow({
  balance,
  now,
}: {
  balance: ZCodePlanBalance;
  now: Date;
}) {
  // A repeating period resets on its period end; a one-time bucket only
  // expires. Only one line is rendered, never both.
  const repeating = balance.period !== undefined && balance.period !== "one_time";
  const resetAt = repeating ? balance.periodEnd : undefined;
  const expiresAt = repeating ? undefined : balance.expiresAt;
  return (
    <div className="zcode-balance">
      <div className="limit-row">
        <span className="limit-label">{balanceLabel(balance)}</span>
        <span className="limit-percent zcode-balance-value">
          {formatBalanceValue(balance)}
        </span>
      </div>
      {resetAt ? (
        <span className="limit-reset">Resets {formatResetTime(resetAt, now)}</span>
      ) : null}
      {expiresAt ? (
        <span className="limit-reset">Expires {formatResetTime(expiresAt, now)}</span>
      ) : null}
    </div>
  );
}
