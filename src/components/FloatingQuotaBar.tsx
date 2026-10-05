import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { FloatingQuotaItem } from "../lib/floatingQuota";
import type { SmartDockGesture } from "../lib/smartDocking";
import { clampPopoverAlign, floatingClickAction } from "../lib/floatingQuota";
import { FloatingProviderItem } from "./FloatingProviderItem";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

/** One card width for hover previews and pinned clicks alike. */
const POPOVER_WIDTH = 300;

/** Where the active-provider selector sits inside the pill. */
type SelectorSpot = { left: number; width: number; shown: boolean };

/**
 * The resting dock's lifecycle, mirrored onto the root for CSS. While
 * resting the strip is the whole visible surface and the pill is hidden;
 * while revealing the strip fades out over the pill's entrance; "none" is
 * the undocked production bar, untouched by dock styling.
 */
export type FloatingDockState = "none" | "resting" | "revealing" | "revealed";

export function FloatingQuotaBar({
  items,
  loading,
  now,
  detailId,
  pinned,
  menuOpen,
  alwaysOnTop,
  note,
  exiting,
  onHover,
  onHoverEnd,
  onHoverCancel,
  onOpen,
  onContextMenu,
  onDismiss,
  onOpenMain,
  onRefresh,
  onTogglePin,
  onHide,
  onDetailCardHeight,
  dockState = "none",
  dockSlot = null,
  onPointerEnterWindow,
  onPointerLeaveWindow,
  onDockToggle,
  docked = false,
  dockGesture = null,
}: {
  items: FloatingQuotaItem[];
  loading: boolean;
  now: Date;
  detailId: string | null;
  pinned: boolean;
  menuOpen: boolean;
  alwaysOnTop: boolean;
  note: string | null;
  exiting: boolean;
  onHover: (id: string) => void;
  onHoverEnd: () => void;
  onHoverCancel: () => void;
  onOpen: (id: string) => void;
  onContextMenu: () => void;
  onDismiss: () => void;
  onOpenMain: () => void;
  onRefresh: () => void;
  onTogglePin: () => void;
  onHide: () => void;
  onDetailCardHeight: (height: number | null) => void;
  /** Resting dock lifecycle for CSS; "none" leaves production styling. */
  dockState?: FloatingDockState;
  /** The resting meter strip, rendered under the pill inside the drag root. */
  dockSlot?: ReactNode;
  /** Pointer entered/left the whole floating window (dock reveal/collapse). */
  onPointerEnterWindow?: () => void;
  onPointerLeaveWindow?: () => void;
  /** Present only while the dock feature is active: the additive menu action. */
  onDockToggle?: () => void;
  docked?: boolean;
  /** Transient snap/unsnap cue. It does not change the persisted dock mode. */
  dockGesture?: SmartDockGesture | null;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [align, setAlign] = useState(0);
  const [selector, setSelector] = useState<SelectorSpot>({
    left: 0,
    width: 0,
    shown: false,
  });
  const selectorRef = useRef(selector);
  selectorRef.current = selector;
  const placeTimer = useRef<number | null>(null);
  const active = items.find((item) => item.providerId === detailId) ?? null;
  const activeId = active?.providerId ?? null;

  useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!activeId || !shell) {
      // Closed: drop the shown flag so the next open lands silently at its
      // segment instead of gliding in from wherever the selector last sat.
      if (placeTimer.current !== null) {
        window.clearTimeout(placeTimer.current);
        placeTimer.current = null;
      }
      setSelector((current) =>
        current.shown ? { ...current, shown: false } : current,
      );
      return;
    }
    const item = shell.querySelector<HTMLElement>(
      '[data-provider-id="' + activeId + '"]',
    );
    if (!item) return;
    // The selector is absolutely positioned inside the shell's padding box
    // while rects are border-box based, so the shell border comes off.
    const shellRect = shell.getBoundingClientRect();
    const itemRect = item.getBoundingClientRect();
    const left = itemRect.left - shellRect.left - shell.clientLeft;
    const width = itemRect.width;
    setAlign(
      clampPopoverAlign(item.offsetLeft, shell.clientWidth, POPOVER_WIDTH),
    );
    const current = selectorRef.current;
    if (!current.shown) {
      // First placement (or a move while hidden): land silently, then fade
      // in on the next tick — appearing, never sweeping in.
      if (placeTimer.current !== null) window.clearTimeout(placeTimer.current);
      placeTimer.current = window.setTimeout(() => {
        placeTimer.current = null;
        setSelector((spot) => (spot.shown ? spot : { ...spot, shown: true }));
      }, 0);
      setSelector({ left, width, shown: false });
    } else if (current.left !== left || current.width !== width) {
      setSelector({ left, width, shown: true });
    }
  }, [activeId, items.length]);

  useEffect(
    () => () => {
      if (placeTimer.current !== null) window.clearTimeout(placeTimer.current);
    },
    [],
  );

  // The window reserves exactly the rendered card's height, so a tall card
  // (many quota windows, an error line) can never overflow the window — that
  // overflow is what let the document scroll the pill out of view. jsdom has
  // no layout, so a zero measurement reports nothing and the window keeps
  // its static fallback headroom.
  const reportCardHeight = useCallback(
    (card: Element | null) => {
      if (!card) return;
      const height = card.getBoundingClientRect().height;
      onDetailCardHeight(height > 0 ? Math.ceil(height) : null);
    },
    [onDetailCardHeight],
  );

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const card = root.querySelector(".fq-popover");
    if (!activeId || !card) {
      onDetailCardHeight(null);
      return;
    }
    reportCardHeight(card);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => reportCardHeight(card));
    observer.observe(card);
    return () => {
      observer.disconnect();
      onDetailCardHeight(null);
    };
  }, [activeId, reportCardHeight, onDetailCardHeight]);

  useEffect(() => {
    if (!menuOpen) return;
    const button = menuRef.current?.querySelector("button");
    if (button instanceof HTMLButtonElement) button.focus();
    // Capture runs before Tauri's document drag listener can consume a hit.
    const outside = (event: MouseEvent) => {
      if (event.button !== 0) return;
      if (event.target instanceof Node && menuRef.current?.contains(event.target)) return;
      onDismiss();
    };
    const blur = () => onDismiss();
    document.addEventListener("mousedown", outside, true);
    window.addEventListener("blur", blur);
    return () => {
      document.removeEventListener("mousedown", outside, true);
      window.removeEventListener("blur", blur);
    };
  }, [menuOpen, onDismiss]);

  return (
    <div
      ref={rootRef}
      className="fq-root"
      data-tauri-drag-region="deep"
      data-dock-state={dockState}
      data-dock-gesture={dockGesture ?? undefined}
      onMouseEnter={onPointerEnterWindow}
      onMouseLeave={onPointerLeaveWindow}
      onMouseDown={(event) => {
        if (event.button !== 0) return;
        const target = event.target;
        if (!(target instanceof Element)) return;
        if (target.closest("button, .fq-popover, .fq-menu")) return;
        onDismiss();
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        const target = event.target;
        if (target instanceof Element && target.closest(".fq-menu, .fq-popover")) return;
        onContextMenu();
      }}
    >
      {dockSlot}
      <div className="fq-shell" data-tauri-drag-region="deep" ref={shellRef}>
        <span
          className="fq-active"
          aria-hidden="true"
          data-on={active && selector.shown ? "true" : "false"}
          style={{
            transform: `translateX(${selector.left}px)`,
            width: `${selector.width}px`,
          }}
        />
        <span className="fq-grip" data-tauri-drag-region aria-hidden="true" />
        {items.length === 0 ? (
          <p className="fq-empty">{loading ? "Checking quotas" : "No providers"}</p>
        ) : (
          items.map((item) => (
            <FloatingProviderItem
              key={item.providerId}
              item={item}
              expanded={detailId === item.providerId}
              onHover={onHover}
              onHoverEnd={onHoverEnd}
              onOpen={() => onOpen(floatingClickAction(item).providerId)}
              onContextMenu={onContextMenu}
            />
          ))
        )}
      </div>
      {active ? (
        <FloatingProviderPopover
          item={active}
          pinned={pinned}
          now={now}
          note={note}
          align={align}
          exiting={exiting}
          onMouseEnter={onHoverCancel}
          onMouseLeave={onHoverEnd}
          // Keyboard parity with the pointer: focus entering the card holds
          // it open like hovering it does; focus leaving starts the same
          // graceful close. Internal focus moves are ignored.
          onFocus={(event) => {
            if (
              event.relatedTarget instanceof Node &&
              event.currentTarget.contains(event.relatedTarget)
            ) {
              return;
            }
            onHoverCancel();
          }}
          onBlur={(event) => {
            if (
              event.relatedTarget instanceof Node &&
              event.currentTarget.contains(event.relatedTarget)
            ) {
              return;
            }
            onHoverEnd();
          }}
          onOpenMain={onOpenMain}
        />
      ) : null}
      {menuOpen ? (
        // Plain action group: Tab and Shift+Tab move between the buttons and
        // this is not claimed to be an ARIA menu, so no arrow-key contract.
        <div
          ref={menuRef}
          className="fq-menu"
          data-tauri-drag-region="false"
          role="group"
          aria-label="Floating bar actions"
        >
          <button type="button" onClick={() => { onDismiss(); onRefresh(); }}>
            Refresh
          </button>
          <button type="button" onClick={onOpenMain}>
            Open main window
          </button>
          <button type="button" aria-pressed={alwaysOnTop} onClick={() => { onDismiss(); onTogglePin(); }}>
            Always on top
          </button>
          {onDockToggle ? (
            // Explicit Dock/Undock fallback. A drag can also snap or unsnap.
            <button type="button" onClick={() => { onDismiss(); onDockToggle(); }}>
              {docked ? "Undock from top edge" : "Dock to top edge"}
            </button>
          ) : null}
          <button type="button" onClick={onHide}>
            Hide floating bar
          </button>
        </div>
      ) : null}
    </div>
  );
}
