import type { ProviderUsage } from "../../types";
import { formatResetTime } from "../../lib/format";
import { formatGrokBotPercent, grokBotFor, grokBotLabel } from "../../lib/grokBot";
import { toneFor } from "../../lib/dashboard";

/**
 * Zone B supplement — the Grok Bot (X Premium+) weekly pool observed through
 * the Cursor-served Grok Bot client, rendered as one visually separated,
 * visually subordinate block under the Grok (xAI) card. The regular xAI
 * windows above stay untouched: this block never feeds the percent meters,
 * the strip, or predictions, and its percentage is never summed with
 * "Weekly credits" or "On-demand" — they are different pools from different
 * providers.
 *
 * Only the server-reported facts render: the plan label, the usage
 * percentage, and the weekly reset. Without an observation (missing,
 * stale-trimmed, or for any other provider) this renders nothing, and the
 * existing Grok card is unchanged.
 */
export function GrokBotPanel({ usage, now }: { usage: ProviderUsage; now: Date }) {
  const grokBot = grokBotFor(usage);
  if (!grokBot) return null;
  const label = grokBotLabel(grokBot);
  const tone = toneFor(grokBot.usedPercent);
  return (
    <div className="grok-bot" role="group" aria-label={label}>
      <div className="limit-row">
        <span className="limit-label">{label}</span>
        <span className={`limit-percent tone-${tone}`}>
          {formatGrokBotPercent(grokBot.usedPercent)}
        </span>
      </div>
      {grokBot.resetAt ? (
        <span className="limit-reset">
          Resets {formatResetTime(grokBot.resetAt, now)}
        </span>
      ) : null}
    </div>
  );
}
