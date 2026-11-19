import type { FloatingQuotaItem } from "../lib/floatingQuota";
import { floatingItemLabel, FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import { baseProviderId } from "../lib/providerIds";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";

/**
 * Real provider marks, simplified to single-color silhouettes that read at
 * 16px on the dark pill. OpenAI, Z.ai, and OpenCode trace their published
 * marks; the Antigravity arch and the Grok slash tile are reduced to the
 * filled geometry that makes them recognizable. Claude Code carries
 * Anthropic's published Claude mark geometry, filled with the inherited
 * currentColor instead of the published brand colour. Mistral carries
 * Mistral's published pixel-cat emblem in Mistral's published WHITE
 * monochrome variant, so every mark in this file is a single-colour
 * silhouette. Unknown providers keep the letter fallback. Shared with the
 * detail card header so hover previews and pinned cards carry the same
 * identity as the segment they expand.
 */
export function ProviderMark({ id }: { id: string }) {
  // An account-scoped id (a second OpenAI/Codex account) names the same
  // provider, so it wears the same mark: the mark identifies the provider,
  // never the account.
  const provider = baseProviderId(id);
  if (provider === "opendesign") return <span className="fq-mark-fallback">OD</span>;
  if (provider === "claude-code") {
    // The Claude mark, bundled from Anthropic's published asset: the
    // "claude-logo.svg" file published at
    // https://platform.claude.com/docs/en/manage-claude/oauth-apps/branding-samples
    // ("Download the logo"), copied here as the published path data. The
    // 24x24 viewBox and the geometry are untouched, so the shape is the
    // published mark and not a redraw of it, and nothing is hotlinked at
    // runtime.
    //
    // The fill is the one deliberate departure from the published asset.
    // The brand colour #D97757 is replaced by currentColor, so the mark
    // inherits the muted text-dim of its slot and reads the same in every
    // theme. That monochrome fill is a LimitScope adaptation, not an
    // official approved or licensed colour variant of the mark.
    //
    // Anthropic's trademark guidelines
    // (https://www.anthropic.com/legal/trademark-guidelines, effective
    // Aug 1, 2024) require specific permission and prior approval, forbid
    // altering a trademark (colour, proportion or otherwise), and forbid
    // implying endorsement. Recolouring the mark is a trademark alteration.
    // Permission for this specific use is NOT established by the
    // branding-samples page, which covers the "Link your Claude
    // organization" button for Linked billing products: treat the usage
    // question as a release gate for Human Acceptance, not as a settled
    // licence.
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M11.376 24L10.776 23.544L10.44 22.8L10.776 21.312L11.16 19.392L11.472 17.856L11.76 15.96 L11.928 15.336L11.904 15.288L11.784 15.312L10.344 17.28L8.16 20.232L6.432 22.056 L6.024 22.224L5.304 21.864L5.376 21.192L5.784 20.616L8.16 17.568L9.6 15.672L10.536 14.592 L10.512 14.448H10.464L4.128 18.576L3 18.72L2.496 18.264L2.568 17.52L2.808 17.28L4.704 15.96 L9.432 13.32L9.504 13.08L9.432 12.96H9.192L8.4 12.912L5.712 12.84L3.384 12.744L1.104 12.624 L0.528 12.504L0 11.784L0.048 11.424L0.528 11.112L1.224 11.16L2.736 11.28L5.016 11.424 L6.672 11.52L9.12 11.784H9.504L9.552 11.616L9.432 11.52L9.336 11.424L6.96 9.84L4.416 8.16 L3.072 7.176L2.352 6.672L1.992 6.216L1.848 5.208L2.496 4.488L3.384 4.56L3.6 4.608 L4.488 5.304L6.384 6.768L8.88 8.616L9.24 8.904L9.408 8.808V8.736L9.24 8.472L7.896 6.024 L6.456 3.528L5.808 2.496L5.64 1.872C5.576 1.656 5.544 1.416 5.544 1.152L6.288 0.144001 L6.696 0L7.704 0.144001L8.112 0.504001L8.736 1.92L9.72 4.152L11.28 7.176L11.736 8.088 L11.976 8.904L12.072 9.168H12.24V9.024L12.36 7.296L12.6 5.208L12.84 2.52L12.912 1.752 L13.296 0.840001L14.04 0.360001L14.616 0.624001L15.096 1.32L15.024 1.752L14.76 3.6 L14.184 6.504L13.824 8.472H14.04L14.28 8.208L15.264 6.912L16.92 4.848L17.64 4.032 L18.504 3.12L19.056 2.688H20.088L20.832 3.816L20.496 4.992L19.44 6.336L18.552 7.464 L17.28 9.168L16.512 10.536L16.584 10.632H16.752L19.608 10.008L21.168 9.744L22.992 9.432 L23.832 9.816L23.928 10.2L23.592 11.016L21.624 11.496L19.32 11.952L15.888 12.768 L15.84 12.792L15.888 12.864L17.424 13.008L18.096 13.056H19.728L22.752 13.272L23.544 13.8 L24 14.424L23.928 14.928L22.704 15.528L21.072 15.144L17.232 14.232L15.936 13.92H15.744 V14.016L16.848 15.096L18.84 16.896L21.36 19.224L21.48 19.8L21.168 20.28L20.832 20.232 L18.624 18.552L17.76 17.808L15.84 16.2H15.72V16.368L16.152 17.016L18.504 20.544 L18.624 21.624L18.456 21.96L17.832 22.176L17.184 22.056L15.792 20.136L14.376 17.952 L13.224 16.008L13.104 16.104L12.408 23.352L12.096 23.712L11.376 24Z"
        />
      </svg>
    );
  }
  if (provider === "minimax-token-plan") {
    // The MiniMax mark reduced to a two-letter monogram: the brand's own
    // wordmark is not licensed into the app, and the single-letter fallback
    // would be indistinguishable from other providers at 16px.
    return <span className="fq-mark-fallback">MM</span>;
  }
  if (provider === "mistral") {
    // Mistral's CURRENT emblem: the abstract pixel cat, taken from
    // Mistral's own 2026 brand material at https://mistral.ai/brand/ (the
    // "Icon / Monochrome" entry). The geometry below is the published
    // WHITE vector, copied verbatim:
    //   https://mistral.ai/cms-media/api/media/file/Mistral_Logos_2026.zip
    //     -> Icon/Monochrome/RGB/SVG/Icon-Mistral-White.svg
    //   sha256 65b6edef03c108849cea04b82017882624be60e641f0fa9dff72800b4a6c83b5
    // (the same kit also ships Icon/Gradient/RGB/SVG/Mistral-Icon-Gradient-RGB.svg,
    // sha256 5aa0534fb52009c3ef9321544cd14d4b84ba34d4ee041abb3bdde9c3f2e765ca,
    // whose geometry is identical to the WHITE file's).
    //
    // The published viewBox "0 0 162 162" is kept as published, so the
    // emblem keeps the brand's own clearspace and the ten 20-unit pixels
    // sit exactly where Mistral put them. The white file's ten rects are
    // the gradient file's ten rects with the published white fill, so the
    // shape is the published mark and not a redraw of it. Nothing is
    // cropped, stretched, rotated or redrawn, and the markup is bundled
    // here, so the app never fetches the asset at runtime. The old
    // multicolor "M" logo from mistralai/mistral-common is a retired mark
    // and is deliberately not reproduced.
    //
    // COLOUR: the brand page says "Prefer the gradient version" and
    // "Use monochrome on busy background". The Floating Deck is a row of
    // small marks on a dark surface (graphite #1f1f24, glass
    // rgba(33,33,42,0.55), oled #060607), and every other mark in this
    // file is a monochrome silhouette, so the published WHITE variant is
    // the treatment used here: one colour, no gradient stops, the same
    // pixel-cat shape. The fill is Mistral's published "white", used
    // verbatim, and NOT currentColor: the asset is reproduced as
    // published rather than re-tinted like the currentColor marks.
    //
    // TRADEMARK: https://mistral.ai/brand/ states that Mistral's names,
    // trademarks, logos and icons are Mistral's property, that partners
    // and collaborators are welcome to use the brand, and that the brand
    // must not be stretched, deformed, rotated, relaid out, recoloured or
    // taken from an old or unofficial version. The monochrome variant is
    // the brand's own published file, so nothing here is recoloured or
    // redrawn, but permission for a third-party desktop app to ship this
    // icon is NOT established by that page: treat the usage question as a
    // release gate for Human Acceptance, not as a settled licence.
    // LimitScope is not affiliated with or endorsed by Mistral AI.
    return (
      <svg viewBox="0 0 162 162" aria-hidden="true">
        <rect x="31" y="31" width="20" height="20" fill="white" />
        <rect x="111" y="31" width="20" height="20" fill="white" />
        <rect x="71" y="91" width="20" height="20" fill="white" />
        <rect x="31" y="91" width="20" height="20" fill="white" />
        <rect x="111" y="91" width="20" height="20" fill="white" />
        <rect x="31" y="51" width="40" height="20" fill="white" />
        <rect x="11" y="111" width="60" height="20" fill="white" />
        <rect x="91" y="111" width="60" height="20" fill="white" />
        <rect x="31" y="71" width="100" height="20" fill="white" />
        <rect x="91" y="51" width="40" height="20" fill="white" />
      </svg>
    );
  }
  if (provider === "openai-codex") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"
        />
      </svg>
    );
  }
  if (provider === "zai") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M12.606 1.806l-1.677 2.388c-0.258 0.374-0.697 0.606-1.161 0.606h-9.162V1.794C0.594 1.806 12.606 1.806 12.606 1.806zM24 1.806L9.6 22.206 0 22.206 14.4 1.806zM11.394 22.206l1.69-2.4c0.258-0.374 0.697-0.606 1.161-0.606h9.149v3.006H11.394z"
        />
      </svg>
    );
  }
  if (provider === "opencode-go") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M22 24H2V0h20zM17 4.8H7v14.4h10z" />
      </svg>
    );
  }
  if (provider === "antigravity") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path
          fill="currentColor"
          d="M8 1.9c1.1 0 4.4 5.4 6.2 10.6.3.9.9 1.2.9 1.5 0 .35-.3.5-.75.5-.85 0-1.65-.5-1.9-1.3C11.1 9.4 9.5 6.9 8 6.9S4.9 9.4 3.55 13.2c-.25.8-1.05 1.3-1.9 1.3-.45 0-.75-.15-.75-.5 0-.3.6-.6.9-1.5C3.6 7.3 6.9 1.9 8 1.9Z"
        />
      </svg>
    );
  }
  if (provider === "grok") {
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
  return <span className="fq-mark-fallback">{provider.slice(0, 1).toUpperCase()}</span>;
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
        {/* The "!" badge marks a degraded READING, so it belongs over a
            value. A provider with no reading at all — still loading, or
            whose local source is unavailable — shows a bare em dash, and
            the expanded card states the reason. */}
        {item.state === "error" && item.percent !== null ? (
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
