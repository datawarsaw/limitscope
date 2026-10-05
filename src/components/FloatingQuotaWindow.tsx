import { useCallback, useEffect, useRef, useState } from "react";
import { useNow } from "../hooks/useNow";
import { useProviderUsage } from "../hooks/useProviderUsage";
import { useSettings } from "../hooks/useSettings";
import {
  DOCK_HEIGHT,
  DOCK_WIDTH,
  floatingBarWidth,
  floatingQuotaItems,
  floatingWindowHeight,
  type FloatingOverlay,
} from "../lib/floatingQuota";
import { orderVisibleProviders } from "../lib/providerPreferences";
import {
  attachDockReanchor,
  attachFloatingChrome,
  hideFloatingWindow,
  isTauriRuntime,
  openMainWindow,
  requestGlobalRefresh,
  resizeFloatingWindow,
  setFloatingDock,
  setFloatingPinned,
} from "../lib/floatingWindowChrome";
import { providerSourceNote } from "../lib/v03Integration";
import {
  loadFloatingQuotaPrefs,
  type FloatingDockMode,
} from "../lib/floatingWindowPrefs";
import type { SmartDockGesture } from "../lib/smartDocking";
import { readDevFixture } from "../dev/usageFixtures";
import { FloatingDockStrip } from "./FloatingDockStrip";
import { FloatingQuotaBar } from "./FloatingQuotaBar";

/**
 * The one detail card state: `id` names the provider being shown, `pinned`
 * says a click is holding it open so pointer leaves no longer dismiss it.
 * Hover previews and pinned clicks render the identical card.
 */
type ProviderDetail = { id: string; pinned: boolean };

/** The card's close fade (mirrors .fq-popover.is-exiting in floating.css). */
const DETAIL_EXIT_MS = 120;

/**
 * The reveal window: how long the root reads "revealing" and the strip stays
 * mounted, fading out. It must cover the pill's whole fq-dock-in entrance
 * (220ms in floating.css) with margin, so the flip to "revealed" can never
 * cut the animation mid-flight.
 */
export const DOCK_REVEAL_MS = 240;

/**
 * The dock's whole-window leave grace, mirroring the card's 140ms: a pointer
 * that just crosses the window edge on its way elsewhere must not snap the
 * bar shut.
 */
const DOCK_LEAVE_GRACE_MS = 140;

export function FloatingQuotaWindow() {
  const devOverlay =
    import.meta.env.DEV && typeof window !== "undefined"
      ? new URLSearchParams(window.location.search).get("overlay")
      : null;
  const devFixture =
    import.meta.env.DEV && typeof window !== "undefined"
      ? readDevFixture(window.location.search)
      : null;
  const { settings } = useSettings();
  const { usages: runtimeUsages, loading, refresh } = useProviderUsage(
    settings.refreshIntervalMinutes,
  );
  const usages = devFixture?.usages ?? runtimeUsages;
  const now = new Date(useNow());
  // Presentation only: hidden providers leave the glanceable bar, but the
  // runtime snapshot above stays complete (history, notifications).
  const registryIds = usages.map((usage) => usage.id);
  const visibleUsages = orderVisibleProviders(
    usages,
    (usage) => usage.id,
    registryIds,
    settings.providerPreferences,
  );
  const items = floatingQuotaItems(visibleUsages);
  const [detail, setDetail] = useState<ProviderDetail | null>(
    devOverlay === "hover"
      ? { id: "openai-codex", pinned: false }
      : devOverlay === "quick"
        ? { id: "openai-codex", pinned: true }
        : null,
  );
  const [menuOpen, setMenuOpen] = useState(false);
  // The card that is playing its close fade: detail became null but the card
  // stays mounted for the exit duration so closing reads as a fade, not a pop.
  const [exitDetail, setExitDetail] = useState<ProviderDetail | null>(null);
  const [alwaysOnTop, setAlwaysOnTop] = useState(
    () => loadFloatingQuotaPrefs().alwaysOnTop,
  );
  const [dock, setDock] = useState<FloatingDockMode>(
    () => loadFloatingQuotaPrefs().dock,
  );
  const [dockGesture, setDockGesture] = useState<SmartDockGesture | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [stripExiting, setStripExiting] = useState(false);
  const [pointerInside, setPointerInside] = useState(false);
  // Bumped by DPI/monitor re-anchor events; nudges the resize effect to
  // re-derive the docked anchor from the live monitor state.
  const [anchorTick, setAnchorTick] = useState(0);
  const dockRef = useRef<FloatingDockMode>(dock);
  dockRef.current = dock;
  const leaveTimer = useRef<number | null>(null);
  const exitTimer = useRef<number | null>(null);
  const stripExitTimer = useRef<number | null>(null);
  const overlayOpen = useRef(false);
  const detailRef = useRef<ProviderDetail | null>(detail);
  detailRef.current = detail;
  const pinned = detail?.pinned ?? false;
  // Rendered height of the open detail card; null until measured (and in
  // layout-less test environments), which keeps the static fallback headroom.
  const [detailCardHeight, setDetailCardHeight] = useState<number | null>(null);

  // While the card fades out the window keeps the open layout, so the exit
  // never plays against a collapsing surface.
  const cardDetail = detail ?? exitDetail;
  const exiting = detail === null && cardDetail !== null;
  const overlay: FloatingOverlay = menuOpen ? "menu" : cardDetail ? "detail" : "none";
  overlayOpen.current = overlay !== "none";
  const width = floatingBarWidth(items.length);
  const height = floatingWindowHeight(overlay, detailCardHeight);

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  useEffect(() => {
    const controller = new AbortController();
    let detach = () => {};
    void attachFloatingChrome(() => overlayOpen.current, (next) => {
      // Drag-away undock: the chrome cleared the persisted pref; follow it.
      setDock(next);
      if (next === "top") setAnchorTick((tick) => tick + 1);
      if (next !== "top") {
        setRevealed(false);
        setStripExiting(false);
      }
    }, () => {
      // Native blur observes clicks outside this transparent WebView. A
      // pinned card stays open; only the transient action group dismisses.
      setMenuOpen(false);
      setPointerInside(false);
    }, controller.signal, setDockGesture).then((cleanup) => {
      if (controller.signal.aborted) cleanup();
      else detach = cleanup;
    });
    return () => {
      controller.abort();
      detach();
    };
  }, []);

  // The chrome serializes the constraint and size as one transition.
  const docked = dock === "top";

  // One resize effect drives both placements: undocked it resizes the free
  // bar to the production size; docked it anchors to the top edge of the
  // current monitor's work area instead — the resting strip at a fixed
  // 600×16, the revealed bar at the production size — with the top edge and
  // the horizontal center kept on every change.
  const resting = docked && !revealed;
  useEffect(() => {
    void resizeFloatingWindow(
      resting ? DOCK_WIDTH : width,
      resting ? DOCK_HEIGHT : height,
      overlay !== "none",
      docked,
    );
  }, [width, height, overlay, resting, docked, anchorTick]);

  useEffect(() => {
    if (!docked) return;
    return attachDockReanchor(() => setAnchorTick((tick) => tick + 1));
  }, [docked]);

  const clearLeave = useCallback(() => {
    if (leaveTimer.current !== null) {
      window.clearTimeout(leaveTimer.current);
      leaveTimer.current = null;
    }
  }, []);

  const cancelExit = useCallback(() => {
    if (exitTimer.current !== null) {
      window.clearTimeout(exitTimer.current);
      exitTimer.current = null;
    }
    setExitDetail(null);
  }, []);

  const beginExit = useCallback((ended: ProviderDetail) => {
    if (exitTimer.current !== null) window.clearTimeout(exitTimer.current);
    setExitDetail(ended);
    exitTimer.current = window.setTimeout(() => {
      exitTimer.current = null;
      setExitDetail(null);
    }, DETAIL_EXIT_MS);
  }, []);

  /** Every close path funnels through here. Opening the menu replaces the
   * card outright (its own entrance, no crossfade); otherwise the card fades
   * out over DETAIL_EXIT_MS while the window keeps its size. */
  const closeDetail = useCallback(
    (openMenu: boolean) => {
      const ended = detailRef.current;
      cancelExit();
      setDetail(null);
      setMenuOpen(openMenu);
      if (ended && !openMenu) beginExit(ended);
    },
    [beginExit, cancelExit],
  );

  const dismiss = useCallback(() => {
    clearLeave();
    closeDetail(false);
  }, [clearLeave, closeDetail]);

  const hover = useCallback(
    (id: string) => {
      if (menuOpen || pinned) return;
      clearLeave();
      // A card may still be fading out; re-opening snaps it back instead of
      // letting the fade race the return.
      cancelExit();
      setDetail({ id, pinned: false });
    },
    [cancelExit, clearLeave, menuOpen, pinned],
  );

  const hoverEnd = useCallback(() => {
    if (menuOpen || pinned) return;
    clearLeave();
    leaveTimer.current = window.setTimeout(() => {
      // A click may have pinned the card while the close was pending.
      const current = detailRef.current;
      if (current && !current.pinned) closeDetail(false);
    }, 140);
  }, [clearLeave, closeDetail, menuOpen, pinned]);

  const pinDetail = useCallback(
    (id: string) => {
      clearLeave();
      cancelExit();
      setMenuOpen(false);
      const current = detailRef.current;
      if (current?.pinned && current.id === id) closeDetail(false);
      else setDetail({ id, pinned: true });
    },
    [cancelExit, clearLeave, closeDetail],
  );

  /**
   * Reveal: one native resize to the production size (never animated), then
   * content motion only — the resting surface fades out while the pill fades
   * in, settling 3px into place. The reveal window is the only timed piece;
   * the state itself flips immediately so every production interaction path
   * is live from the first frame.
   */
  const reveal = useCallback(() => {
    if (dockRef.current !== "top") return;
    setRevealed(true);
    if (stripExitTimer.current !== null) window.clearTimeout(stripExitTimer.current);
    setStripExiting(true);
    stripExitTimer.current = window.setTimeout(() => {
      stripExitTimer.current = null;
      setStripExiting(false);
    }, DOCK_REVEAL_MS);
  }, []);

  const onPointerEnterWindow = useCallback(() => {
    setPointerInside(true);
    // Resting: hover reveals. Entering anywhere on the 16px strip grows the
    // window under the pointer, so the pointer lands on the revealed bar.
    reveal();
  }, [reveal]);

  const onPointerLeaveWindow = useCallback(() => {
    setPointerInside(false);
  }, []);

  // Collapse back to the resting strip once the pointer has left the window
  // and nothing is holding the bar open — a pinned card or the menu keeps it
  // revealed until it closes, then the same grace settles the dock. This is
  // the same state machine the card uses, not a second interaction model.
  // The grace is shorter than the reveal window, so a quick graze can settle
  // while the strip is still fading out; the settle cancels that exit so the
  // resting surface is back at full strength instead of fading into a pop.
  useEffect(() => {
    if (dock !== "top" || !revealed || pointerInside || overlay !== "none") {
      return;
    }
    const timer = window.setTimeout(() => {
      if (stripExitTimer.current !== null) {
        window.clearTimeout(stripExitTimer.current);
        stripExitTimer.current = null;
      }
      setStripExiting(false);
      setRevealed(false);
    }, DOCK_LEAVE_GRACE_MS);
    return () => window.clearTimeout(timer);
  }, [dock, revealed, pointerInside, overlay]);

  useEffect(
    () => () => {
      if (stripExitTimer.current !== null) window.clearTimeout(stripExitTimer.current);
    },
    [],
  );

  const toggleDock = useCallback(() => {
    const next: FloatingDockMode = dockRef.current === "top" ? "none" : "top";
    void setFloatingDock(next);
    dockRef.current = next;
    setDock(next);
    setPointerInside(false);
    if (next === "top") {
      // Docking under the pointer: stay revealed so the bar keeps its
      // context, and the leave grace settles it into the resting strip.
      setRevealed(true);
      setStripExiting(false);
      if (stripExitTimer.current !== null) {
        window.clearTimeout(stripExitTimer.current);
        stripExitTimer.current = null;
      }
    } else {
      setRevealed(false);
      setStripExiting(false);
    }
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const returnTo = detailRef.current?.id;
      closeDetail(false);
      if (!returnTo) return;
      const button = document.querySelector<HTMLButtonElement>(
        '[data-provider-id="' + CSS.escape(returnTo) + '"]',
      );
      button?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closeDetail]);

  const refreshAll = useCallback(() => {
    if (isTauriRuntime()) void requestGlobalRefresh();
    else refresh();
  }, [refresh]);

  const togglePin = useCallback(() => {
    const next = !alwaysOnTop;
    setAlwaysOnTop(next);
    void setFloatingPinned(next);
  }, [alwaysOnTop]);

  const activeUsage = usages.find((usage) => usage.id === cardDetail?.id);
  // The popover carries the account identity on its own muted bottom line,
  // so the source note must not repeat it. The same note feeds hover
  // previews and pinned cards — there is only one card.
  const note = activeUsage
    ? providerSourceNote(activeUsage, now, { includeAccount: false })
    : null;

  return (
    <FloatingQuotaBar
      items={items}
      loading={loading}
      now={now}
      detailId={cardDetail?.id ?? null}
      pinned={cardDetail?.pinned ?? false}
      exiting={exiting}
      menuOpen={menuOpen}
      alwaysOnTop={alwaysOnTop}
      note={note}
      onHover={hover}
      onHoverEnd={hoverEnd}
      onHoverCancel={clearLeave}
      onOpen={pinDetail}
      onContextMenu={() => {
        // Right-clicking the resting strip reveals the bar with its menu.
        reveal();
        clearLeave();
        closeDetail(true);
      }}
      onDismiss={dismiss}
      onOpenMain={() => {
        dismiss();
        void openMainWindow();
      }}
      onRefresh={() => {
        refreshAll();
      }}
      onTogglePin={togglePin}
      onHide={() => {
        dismiss();
        void hideFloatingWindow();
      }}
      onDetailCardHeight={setDetailCardHeight}
      dockState={
        !docked
          ? "none"
          : !revealed
            ? "resting"
            : stripExiting
              ? "revealing"
              : "revealed"
      }
      dockSlot={
        docked && (resting || stripExiting) ? (
          <FloatingDockStrip items={items} exiting={stripExiting} />
        ) : null
      }
      onPointerEnterWindow={onPointerEnterWindow}
      onPointerLeaveWindow={onPointerLeaveWindow}
      onDockToggle={toggleDock}
      docked={docked}
      dockGesture={dockGesture}
    />
  );
}
