import type { FloatingQuotaItem } from "../lib/floatingQuota";
import { floatingItemLabel, FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";

/**
 * Real provider marks, simplified to single-color silhouettes that read at
 * 16px on the dark pill. OpenAI, Z.ai, and OpenCode trace their published
 * marks; the Antigravity arch and the Grok slash tile are reduced to the
 * filled geometry that makes them recognizable. Unknown providers keep the
 * letter fallback. Shared with the detail card header so hover previews and
 * pinned cards carry the same identity as the segment they expand.
 */
export function ProviderMark({ id }: { id: string }) {
  if (id === "openai-codex") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"
        />
      </svg>
    );
  }
  if (id === "zai") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M12.606 1.806l-1.677 2.388c-0.258 0.374-0.697 0.606-1.161 0.606h-9.162V1.794C0.594 1.806 12.606 1.806 12.606 1.806zM24 1.806L9.6 22.206 0 22.206 14.4 1.806zM11.394 22.206l1.69-2.4c0.258-0.374 0.697-0.606 1.161-0.606h9.149v3.006H11.394z"
        />
      </svg>
    );
  }
  if (id === "opencode-go") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M22 24H2V0h20zM17 4.8H7v14.4h10z" />
      </svg>
    );
  }
  if (id === "antigravity") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path
          fill="currentColor"
          d="M8 1.9c1.1 0 4.4 5.4 6.2 10.6.3.9.9 1.2.9 1.5 0 .35-.3.5-.75.5-.85 0-1.65-.5-1.9-1.3C11.1 9.4 9.5 6.9 8 6.9S4.9 9.4 3.55 13.2c-.25.8-1.05 1.3-1.9 1.3-.45 0-.75-.15-.75-.5 0-.3.6-.6.9-1.5C3.6 7.3 6.9 1.9 8 1.9Z"
        />
      </svg>
    );
  }
  if (id === "grok") {
    // The Grok slash: the app-tile silhouette with the diagonal slash cut
    // out, reduced from the published mark so it stays crisp at 16px.
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          fillRule="evenodd"
          d="M4.5 0h15A4.5 4.5 0 0 1 24 4.5v15a4.5 4.5 0 0 1-4.5 4.5h-15A4.5 4.5 0 0 1 0 19.5v-15A4.5 4.5 0 0 1 4.5 0Zm10.92 5.06h-2.93L5.68 18.94h2.93L15.42 5.06Z"
        />
      </svg>
    );
  }
  return <span className="fq-mark-fallback">{id.slice(0, 1).toUpperCase()}</span>;
}

export function FloatingProviderItem({
  item,
  expanded,
  onHover,
  onHoverEnd,
  onOpen,
  onContextMenu,
}: {
  item: FloatingQuotaItem;
  expanded: boolean;
  onHover: (id: string) => void;
  onHoverEnd: () => void;
  onOpen: (id: string) => void;
  onContextMenu: () => void;
}) {
  const presentation = floatingQuotaPresentation(
    item.usedPercent ?? item.percent,
    FLOATING_QUOTA_PERSPECTIVE,
  );
  const percent = presentation.displayPercentText;
  return (
    <button
      type="button"
      className={"fq-item is-" + item.state + (item.stale ? " is-marked-stale" : "")}
      data-provider={item.providerId}
      data-provider-id={item.providerId}
      data-state={item.state}
      data-quota={quotaColorLevel(presentation)}
      aria-label={floatingItemLabel(item)}
      aria-haspopup="dialog"
      aria-expanded={expanded}
      onMouseEnter={() => onHover(item.providerId)}
      onMouseLeave={onHoverEnd}
      onFocus={() => onHover(item.providerId)}
      onBlur={onHoverEnd}
      onClick={() => onOpen(item.providerId)}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onContextMenu();
      }}
    >
      <span className="fq-mark">
        <ProviderMark id={item.providerId} />
      </span>
      <span className="fq-readout">
        <span className="fq-percent">{percent}</span>
        {item.state === "error" ? (
          <span className="fq-warn" aria-hidden="true">
            !
          </span>
        ) : null}
        {item.stale ? <span className="fq-stale-dot" aria-hidden="true" /> : null}
      </span>
      <span
        className="fq-meter"
        role="progressbar"
        aria-label={`${item.name} quota · ${presentation.ariaLabel}`}
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
    </button>
  );
}
