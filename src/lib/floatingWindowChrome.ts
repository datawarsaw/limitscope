import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  availableMonitors,
  currentMonitor,
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
  PhysicalSize,
  primaryMonitor,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  createFloatingDragLifecycleConsumer,
  FLOATING_DRAG_LIFECYCLE_EVENT,
  type FloatingDragLifecycleEvent,
} from "./floatingDragLifecycle";
import {
  DOCK_HEIGHT,
  DOCK_WIDTH,
  FLOATING_WINDOW_HEIGHT,
} from "./floatingQuota";
import {
  clampDockAnchor,
  createTrailingPositionSave,
  isFloatingPositionVisible,
  loadFloatingQuotaPrefs,
  saveFloatingQuotaPrefs,
  type FloatingDockMode,
} from "./floatingWindowPrefs";
import { clampWindowBoundsToWorkArea } from "./windowBounds";
import {
  evaluateSmartDock,
  gestureFromPhase,
  selectDragMonitor,
  type SmartDockGesture,
  type SmartDockMonitor,
  type SmartDockPhase,
} from "./smartDocking";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Webview → Rust announcement that keeps the tray label in sync (A10). */
export const FLOATING_VISIBILITY_EVENT = "floating://visible-changed";

// Ignore only the echo of a position we requested. A time blanket also
// swallowed real drags immediately after reveal, focus or menu dismissal.
const requestedMoves: Array<{ x: number; y: number; expires: number }> = [];
let dockGeometry: { width: number; monitor: NonNullable<Awaited<ReturnType<typeof currentMonitor>>> } | null = null;
let resizeQueue: Promise<void> = Promise.resolve();
// A mode commit owns all of its queued placements. Size-only transitions
// within that intent remain serialized; Dock/Undock invalidate older work.
let placementRevision = 0;
let freePosition: { x: number; y: number } | null = null;
let dockOrigin: { x: number; y: number } | null = null;
let dockMovePending: Promise<void> = Promise.resolve();
let parked: { x: number; y: number; shiftedX: number; shiftedY: number } | null = null;
type NativeDragSession = {
  generation: number;
  gesture: number;
  revision: number;
  ownsPlacement: boolean;
  streamActive: boolean;
  origin: {
    dock: FloatingDockMode;
    dockAnchor: number | null;
    x: number | null;
    y: number | null;
    position: { x: number; y: number };
  };
  pendingPosition: { x: number; y: number } | null;
  lastAppliedPosition: { x: number; y: number } | null;
  moveWorker: Promise<void> | null;
  smartDock: { x: number; y: number; phase: SmartDockPhase } | null;
};
let nativeDragSession: NativeDragSession | null = null;
let gesturePreviewListener: ((gesture: SmartDockGesture | null) => void) | null = null;
let publishedGesture: SmartDockGesture | null = null;
let deferredResize: {
  width: number;
  height: number;
  overlay: boolean;
  docked: boolean;
  revision: number;
} | null = null;

function ownsPlacement(revision: number, docked: boolean): boolean {
  return revision === placementRevision &&
    (loadFloatingQuotaPrefs().dock === "top") === docked;
}

function ownsNativeDragSession(session: NativeDragSession): boolean {
  return session.ownsPlacement && nativeDragSession === session &&
    session.revision === placementRevision;
}

function publishGesturePreview(gesture: SmartDockGesture | null) {
  if (publishedGesture === gesture) return;
  publishedGesture = gesture;
  gesturePreviewListener?.(gesture);
}

function toSmartMonitor(
  monitor: NonNullable<Awaited<ReturnType<typeof currentMonitor>>>,
): SmartDockMonitor {
  return {
    scaleFactor: monitor.scaleFactor,
    workArea: {
      x: monitor.workArea.position.x,
      y: monitor.workArea.position.y,
      width: monitor.workArea.size.width,
      height: monitor.workArea.size.height,
    },
  };
}

async function positionFloatingWindow(
  x: number,
  y: number,
  isCurrent: () => boolean = () => true,
): Promise<boolean> {
  if (!isCurrent()) return false;
  const now = Date.now();
  for (let i = requestedMoves.length - 1; i >= 0; i--) {
    if (requestedMoves[i].expires < now) requestedMoves.splice(i, 1);
  }
  const move = { x, y, expires: now + 1000 };
  requestedMoves.push(move);
  if (!isCurrent()) {
    requestedMoves.splice(requestedMoves.indexOf(move), 1);
    return false;
  }
  try {
    await getCurrentWindow().setPosition(new PhysicalPosition(x, y));
  } catch (error) {
    const index = requestedMoves.indexOf(move);
    if (index >= 0) requestedMoves.splice(index, 1);
    throw error;
  }
  // An IPC call already submitted before a new drag cannot be recalled, but
  // it must not retain an echo token or trigger any follow-up placement work.
  if (!isCurrent()) {
    const index = requestedMoves.indexOf(move);
    if (index >= 0) requestedMoves.splice(index, 1);
    return false;
  }
  return true;
}

function isRequestedMove(x: number, y: number): boolean {
  const now = Date.now();
  for (let i = requestedMoves.length - 1; i >= 0; i--) {
    if (requestedMoves[i].expires < now) requestedMoves.splice(i, 1);
  }
  const index = requestedMoves.findIndex((move) => move.x === x && move.y === y);
  if (index < 0) return false;
  requestedMoves.splice(index, 1);
  return true;
}

/** How long the window must rest after a drag before its position is stored. */
export const FLOATING_MOVE_SAVE_DELAY_MS = 200;

export async function openMainWindow(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("open_main_window");
}

export async function requestGlobalRefresh(): Promise<void> {
  if (!isTauriRuntime()) return;
  // The shared runtime is the single refresh owner; snapshot events update
  // every window from the one completed cycle.
  await invoke("request_refresh");
}

/**
 * Announces the floating bar's visibility so the tray can mirror it (A10).
 * The tray decides its toggle action from the real window state at click
 * time, so a lost announcement only leaves a stale label, never a wrong
 * action. Best-effort: the bar keeps working when the event cannot be sent.
 */
export async function announceFloatingVisibility(visible: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await emit(FLOATING_VISIBILITY_EVENT, visible);
  } catch (error) {
    console.warn("Floating window could not announce its visibility", error);
  }
}

/**
 * A09: after the floating window hides itself, hand focus back to the main
 * window — but only when the main window is visible. When it is hidden the
 * OS prior-foreground naturally retains focus, and stealing focus would pop
 * the dashboard over an unrelated app.
 */
export async function restoreMainWindowFocus(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const main = await WebviewWindow.getByLabel("main");
    if (!main || !(await main.isVisible())) return;
    await main.setFocus();
  } catch (error) {
    console.warn("Floating window could not restore main-window focus", error);
  }
}

export async function hideFloatingWindow(): Promise<void> {
  const prefs = loadFloatingQuotaPrefs();
  saveFloatingQuotaPrefs({ ...prefs, visible: false });
  if (!isTauriRuntime()) return;
  const win = getCurrentWindow();
  try {
    await win.hide();
  } catch (error) {
    console.warn("Floating window could not hide itself", error);
  }
  await announceFloatingVisibility(false);
  await restoreMainWindowFocus();
}

export async function setFloatingPinned(pinned: boolean): Promise<void> {
  const prefs = loadFloatingQuotaPrefs();
  saveFloatingQuotaPrefs({ ...prefs, alwaysOnTop: pinned });
  if (!isTauriRuntime()) return;
  await getCurrentWindow().setAlwaysOnTop(pinned);
}

/**
 * Persists the resting dock state. The window itself is re-anchored by the
 * floating window's resize effect, which reads the new value; this only
 * owns the preference and invalidates older native placements. Undock also
 * gives the next free resize the persisted free point to reconcile.
 */
export async function setFloatingDock(dock: FloatingDockMode): Promise<void> {
  // An explicit menu choice wins over an in-flight gesture. Keep its stream
  // filtered until native ends, but make every stale session callback inert.
  if (nativeDragSession) nativeDragSession.ownsPlacement = false;
  publishGesturePreview(null);
  // This resize was derived from the invalidated gesture. React's mode
  // update supplies the replacement intent; do not resize with its old size.
  deferredResize = null;
  placementRevision++;
  const prefs = loadFloatingQuotaPrefs();
  parked = null;
  if (dock === "top" && prefs.dock !== "top") dockOrigin = null;
  freePosition = dock === "none" && prefs.x !== null && prefs.y !== null
    ? { x: prefs.x, y: prefs.y } : dock === "none" ? dockOrigin : null;
  saveFloatingQuotaPrefs({ ...prefs, dock });
}

/**
 * The free-floating bar's programmatic size floor, mirroring the
 * floating-quota window's tauri.conf.json constraints (minWidth 300,
 * minHeight = the 64px pill window). The resting dock must resize to 16px
 * tall, below that floor, so the constraint is relaxed while docked and
 * restored on undock.
 */
const FREE_FLOATING_MIN = { width: 300, height: FLOATING_WINDOW_HEIGHT };

/**
 * Relaxes the window's min-size constraint while the resting dock needs the
 * 16px strip, and restores the production floor for the free bar. Best-effort:
 * a missed call only leaves the window clamped to the free bar's floor.
 */
export async function setFloatingDockConstraints(docked: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await getCurrentWindow().setMinSize(
      docked ? null : new LogicalSize(FREE_FLOATING_MIN.width, FREE_FLOATING_MIN.height),
    );
  } catch (error) {
    console.warn("Floating window could not apply its size constraints", error);
  }
}

/**
 * Drag-away undock threshold, in physical px: the dock leaves the top edge
 * once the window top has been dragged further than the strip's own height.
 * A deliberate pull; jitter and window-manager rounding stay under it.
 */
export function undockThresholdPx(scaleFactor: number): number {
  return Math.max(1, Math.round(DOCK_HEIGHT * (scaleFactor || 1)));
}

/**
 * Whether a moved position crosses the undock threshold for the work area it
 * sits in. Pure; the onMoved handler applies the pref change.
 */
export function shouldUndockFromTop(
  y: number,
  workAreaTop: number,
  scaleFactor: number,
): boolean {
  return y - workAreaTop > undockThresholdPx(scaleFactor);
}

// ---------- Docked horizontal anchor ----------

type WorkAreaSpan = { x: number; width: number };

/** Physical center x of a dock anchor fraction within a work area. */
export function dockAnchorCenterX(anchor: number, workArea: WorkAreaSpan): number {
  return workArea.x + (0.5 + clampDockAnchor(anchor)) * workArea.width;
}

/** Anchor fraction for a physical window center x, clamped into range. */
export function dockAnchorFromCenter(
  centerX: number,
  workArea: WorkAreaSpan,
): number {
  if (!Number.isFinite(centerX) || workArea.width <= 0) return 0;
  return clampDockAnchor(
    (centerX - (workArea.x + workArea.width / 2)) / workArea.width,
  );
}

/**
 * The docked window's shared horizontal center for an anchor fraction. Both
 * the resting strip and the revealed bar center on this x, so the reveal
 * never slides the meters horizontally. The center is confined to the range
 * where the widest docked placement — the 600px resting strip — still fits
 * the work area, so near an edge both placements clamp identically; a work
 * area narrower than that falls back to the middle and the per-placement
 * clamp owns the edges.
 */
export function dockCenterForAnchor(
  anchor: number,
  workArea: WorkAreaSpan,
  widestHalf: number,
): number {
  const min = workArea.x + widestHalf;
  const max = workArea.x + workArea.width - widestHalf;
  if (min > max) return workArea.x + workArea.width / 2;
  return Math.min(Math.max(dockAnchorCenterX(anchor, workArea), min), max);
}

type DockedMovePayload = { x: number; y: number };

/**
 * Clears the dock pref (and reports it) when a drag moves the docked window
 * away from the top edge of its monitor. Resolves without acting when not
 * docked or still within the threshold.
 */
async function handleDockedMove(
  payload: DockedMovePayload,
  onDockChange: ((dock: FloatingDockMode) => void) | undefined,
  revision: number,
  width: number | undefined,
  isCurrent: () => boolean,
  options?: {
    deferAnchorSave?: boolean;
    deferPositionSave?: boolean;
    onPlacementRevision?: (revision: number) => void;
  },
): Promise<void> {
  if (loadFloatingQuotaPrefs().dock !== "top") return;
  try {
    const monitor = (await currentMonitor()) ?? (await primaryMonitor());
    if (!monitor || !ownsPlacement(revision, true) || !isCurrent()) return;
    const undocks = shouldUndockFromTop(
      payload.y,
      monitor.workArea.position.y,
      monitor.scaleFactor,
    );
    const current = loadFloatingQuotaPrefs();
    if (current.dock !== "top") return;
    if (!undocks) {
      if (width !== undefined) {
        if (!options?.deferAnchorSave) {
          saveFloatingQuotaPrefs({ ...current, dockAnchor: dockAnchorFromCenter(
            payload.x + width / 2,
            { x: monitor.workArea.position.x, width: monitor.workArea.size.width },
          ) });
        }
        dockGeometry = { width: dockGeometry?.width ?? width, monitor };
      }
      return;
    }
    placementRevision++;
    options?.onPlacementRevision?.(placementRevision);
    parked = null;
    freePosition = { x: payload.x, y: payload.y };
    saveFloatingQuotaPrefs(options?.deferPositionSave
      ? { ...current, dock: "none" }
      : { ...current, dock: "none", x: payload.x, y: payload.y });
    onDockChange?.("none");
  } catch (error) {
    console.warn("Floating window could not evaluate the dock state", error);
  }
}

/**
 * Resolves a docked drag's settled left edge into the work-area-relative
 * anchor fraction and persists it — the horizontal counterpart of the
 * derived top-edge anchor. The dock pref is re-read after the monitor and
 * size awaits so a drag that undocked while the trailing save was pending
 * drops its write: the anchor can follow the dock, never lead it back.
 * Free-bar x/y stay untouched while docked.
 */
async function persistDockAnchor(x: number): Promise<void> {
  if (!isTauriRuntime()) return;
  const revision = placementRevision;
  try {
    if (!ownsPlacement(revision, true)) return;
    const [monitor, size] = await Promise.all([
      currentMonitor().then((found) => found ?? primaryMonitor()),
      getCurrentWindow().outerSize(),
    ]);
    if (!monitor) return;
    if (!ownsPlacement(revision, true)) return;
    const anchor = dockAnchorFromCenter(x + size.width / 2, {
      x: monitor.workArea.position.x,
      width: monitor.workArea.size.width,
    });
    const current = loadFloatingQuotaPrefs();
    saveFloatingQuotaPrefs({ ...current, dockAnchor: anchor });
  } catch (error) {
    console.warn("Floating window could not store its dock position", error);
  }
}

/**
 * Feature switch for the floating bar (v0.6). Persisting the flag is all
 * this does: geometry and pin stay exactly as stored, so re-enabling
 * restores the bar where it was. Hiding the window is the separate,
 * weaker hideFloatingWindow action and must never touch this flag. Tray
 * and settings surfaces gate their show actions on the persisted value.
 */
export async function setFloatingBarEnabled(enabled: boolean): Promise<void> {
  const prefs = loadFloatingQuotaPrefs();
  saveFloatingQuotaPrefs({ ...prefs, floatingBarEnabled: enabled });
}

/**
 * Restores pin, position, and visibility once. Returns a cleanup that
 * drops the window listeners. Size is owned by resizeFloatingWindow.
 *
 * `onDockChange` reports the drag-away undock so the floating window's React
 * state can follow the persisted pref. On the native gesture path, snap and
 * unsnap stay transient until release. The legacy generation-0 moved stream
 * still commits a drag-away undock during the move, because that stream has
 * no terminal event. A settled docked drag writes only the horizontal anchor.
 * onGesturePreview reports the content-only preview and never a second window.
 */
export async function attachFloatingChrome(
  overlayOpen: () => boolean,
  onDockChange?: (dock: FloatingDockMode) => void,
  onBlur?: () => void,
  signal?: AbortSignal,
  onGesturePreview?: (gesture: SmartDockGesture | null) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => {};
  publishedGesture = null;
  gesturePreviewListener = onGesturePreview ?? null;
  const win = getCurrentWindow();
  const prefs = loadFloatingQuotaPrefs();
  const revision = placementRevision;
  const unlistens: UnlistenFn[] = [];
  requestedMoves.length = 0;
  dockGeometry = null;

  try {
    await win.setAlwaysOnTop(prefs.alwaysOnTop);
  } catch (error) {
    console.warn("Floating window could not apply always-on-top", error);
  }

  try {
    // While docked the position is derived from the current monitor's work
    // area (the docked resize path anchors it); the stored free-bar position
    // must not fight that anchor, but the stored point still names the
    // monitor the OS placed us on, which is the one the dock belongs to.
    if (prefs.dock !== "top" && prefs.x !== null && prefs.y !== null) {
      const savedPosition = { x: prefs.x, y: prefs.y };
      const restore = resizeQueue.then(async () => {
        if (!ownsPlacement(revision, false)) return;
        const monitors = await availableMonitors();
        if (!ownsPlacement(revision, false)) return;
        const size = await win.outerSize();
        const visible = isFloatingPositionVisible(
          savedPosition.x,
          savedPosition.y,
          size.width,
          size.height,
          monitors.map((monitor) => ({
            x: monitor.workArea.position.x,
            y: monitor.workArea.position.y,
            width: monitor.workArea.size.width,
            height: monitor.workArea.size.height,
          })),
        );
        if (visible && ownsPlacement(revision, false)) {
          await positionFloatingWindow(savedPosition.x, savedPosition.y);
        }
      });
      resizeQueue = restore.catch(() => {});
      await restore;
    }
  } catch (error) {
    console.warn("Floating window could not restore its position", error);
  }

  if (prefs.dock === "top") {
    const [monitor, size] = await Promise.all([currentMonitor(), win.outerSize()]);
    if (monitor && !dockGeometry && ownsPlacement(revision, true)) {
      dockGeometry = { monitor, width: size.width };
    }
  }

  const savePosition = createTrailingPositionSave(
    FLOATING_MOVE_SAVE_DELAY_MS,
    (x, y) => {
      const current = loadFloatingQuotaPrefs();
      if (current.dock === "top") return;
      saveFloatingQuotaPrefs({ ...current, x, y });
    },
  );

  // The docked counterpart: while docked, settled drags persist the
  // work-area-relative anchor instead of the free-bar position. The y is
  // ignored — the top edge stays derived, and only handleDockedMove may
  // clear the dock.
  const saveDockAnchor = createTrailingPositionSave(
    FLOATING_MOVE_SAVE_DELAY_MS,
    (x) => {
      if (loadFloatingQuotaPrefs().dock !== "top") return;
      const revision = placementRevision;
      if (dockGeometry) onDockChange?.("top");
      else void persistDockAnchor(x).then(() => {
        if (ownsPlacement(revision, true)) onDockChange?.("top");
      });
    },
  );
  let moveRevision = 0;
  let lifecycleGeneration = 0;
  // A native generation makes lifecycle events authoritative. Only the
  // documented non-Windows generation 0 may use Tao's legacy moved stream.
  let legacyMovedPersistence = false;
  let disposed = false;
  let abortRequested = signal?.aborted ?? false;
  let cleanup: () => void = () => {};
  const abort = () => {
    abortRequested = true;
    cleanup();
  };
  signal?.addEventListener("abort", abort, { once: true });

  const flushDeferredResize = () => {
    const pending = deferredResize;
    deferredResize = null;
    if (!pending || pending.revision !== placementRevision) return;
    void resizeFloatingWindow(
      pending.width,
      pending.height,
      pending.overlay,
      loadFloatingQuotaPrefs().dock === "top",
    );
  };

  const concludeNativeDrag = (session: NativeDragSession) => {
    session.streamActive = false;
    if (nativeDragSession === session) nativeDragSession = null;
    publishGesturePreview(null);
    flushDeferredResize();
  };

  const cancelNativeDrag = (session: NativeDragSession) => {
    if (!session.streamActive) return;
    const ownsOrigin = ownsNativeDragSession(session);
    session.streamActive = false;
    if (nativeDragSession === session) nativeDragSession = null;
    session.ownsPlacement = false;
    session.pendingPosition = null;
    publishGesturePreview(null);
    if (!ownsOrigin) {
      flushDeferredResize();
      return;
    }

    // This session reserved placementRevision at begin. Restore the durable
    // placement immediately, before a new begin can snapshot the transient
    // drag-away Undock. An explicit Dock/Undock after this point owns a newer
    // revision and therefore suppresses only the queued native geometry.
    const restoreRevision = ++placementRevision;
    // A deferred free-bar resize from a drag-away must not become a docked
    // resize merely because cancellation restored the dock. Keep only a
    // pending resize that already describes the restored placement.
    if (deferredResize) {
      if (deferredResize.docked === (session.origin.dock === "top")) {
        deferredResize.revision = restoreRevision;
      } else {
        deferredResize = null;
      }
    }
    const ownsRestore = () => placementRevision === restoreRevision;
    parked = null;
    dockGeometry = null;
    freePosition = session.origin.dock === "none" &&
      session.origin.x !== null && session.origin.y !== null
      ? { x: session.origin.x, y: session.origin.y }
      : null;
    const current = loadFloatingQuotaPrefs();
    saveFloatingQuotaPrefs({
      ...current,
      dock: session.origin.dock,
      dockAnchor: session.origin.dockAnchor,
      x: session.origin.x,
      y: session.origin.y,
    });
    onDockChange?.(session.origin.dock);
    // Join the existing queue: an earlier resize may already be between
    // awaits. Native accepts the restore only for the exact ended gesture;
    // a newer native gesture therefore cannot be jumped back by this stale
    // cancellation even before its begin event reaches this WebView.
    const restore = resizeQueue.then(async () => {
      if (!ownsRestore()) return;
      const restored = session.generation > 0
        ? await invoke<boolean>("restore_floating_drag_origin", {
          generation: session.generation,
          gesture: session.gesture,
          x: session.origin.position.x,
          y: session.origin.position.y,
        })
        : await positionFloatingWindow(
          session.origin.position.x,
          session.origin.position.y,
          ownsRestore,
        );
      // False means native observed a newer or still-active gesture. Do not
      // flush a deferred resize because JS may not have received that begin.
      if (restored && ownsRestore()) flushDeferredResize();
    });
    resizeQueue = restore.catch(() => {});
    void restore.catch((error) => {
      console.warn("Floating window could not restore a cancelled drag", error);
    });
  };

  const evaluateNativeSmartDock = async (
    payload: DockedMovePayload,
    session: NativeDragSession,
    knownWidth: number | undefined,
    isCurrent: () => boolean,
  ) => {
    try {
      // Start the other reads before the monitor await. When that await is
      // the long one, they are already done, so the worker can register the
      // next lookup in the same turn. An extra await here drops a coalesced
      // terminal point.
      let monitors: Awaited<ReturnType<typeof availableMonitors>> = [];
      let sizeWidth = knownWidth;
      let monitorsDone = false;
      let sizeDone = knownWidth !== undefined && session.origin.dock === "top";
      const monitorsPromise = availableMonitors().then((value) => {
        monitors = value;
        monitorsDone = true;
      });
      const sizePromise = sizeDone
        ? Promise.resolve()
        : win.outerSize().then((size) => {
            sizeWidth = size.width;
            sizeDone = true;
          });
      const found = await currentMonitor();
      if (!isCurrent()) return;
      const current = found ?? await primaryMonitor();
      if (!monitorsDone || !sizeDone) await Promise.all([monitorsPromise, sizePromise]);
      if (!isCurrent() || sizeWidth === undefined) return;
      const snapshots = monitors.map(toSmartMonitor);
      const fallback = current ? toSmartMonitor(current) : null;
      const point = { x: payload.x + sizeWidth / 2, y: payload.y };
      const monitor = selectDragMonitor(snapshots, point) ?? fallback;
      if (!monitor || !isCurrent()) return;
      const phase = evaluateSmartDock({
        stable: session.origin.dock === "top" ? "docked" : "floating",
        x: payload.x,
        y: payload.y,
        width: sizeWidth,
        monitor,
      });
      if (!isCurrent()) return;
      session.smartDock = { x: payload.x, y: payload.y, phase };
      publishGesturePreview(gestureFromPhase(phase));
    } catch (error) {
      console.warn("Floating window could not evaluate smart docking", error);
    }
  };

  const applyDraggedPosition = (
    payload: DockedMovePayload,
    session?: NativeDragSession,
  ) => {
    if (session && !ownsNativeDragSession(session)) return;
    const moved = ++moveRevision;
    const width = dockGeometry?.width;
    if (session) {
      // Native gestures preview only. FLOATING and DOCKED commit on release.
      dockMovePending = evaluateNativeSmartDock(
        payload,
        session,
        width,
        () => moved === moveRevision && ownsNativeDragSession(session),
      );
      return;
    }
    const revision = placementRevision;
    dockMovePending = handleDockedMove(
      payload,
      onDockChange,
      revision,
      width,
      () => moved === moveRevision,
    );
    if (loadFloatingQuotaPrefs().dock === "top") {
      // Store the center before a leave/reveal resize changes the width.
      // This is synchronous so an immediate collapse uses the new anchor.
      if (dockGeometry && !shouldUndockFromTop(payload.y,
        dockGeometry.monitor.workArea.position.y, dockGeometry.monitor.scaleFactor)) {
        const current = loadFloatingQuotaPrefs();
        saveFloatingQuotaPrefs({ ...current, dockAnchor: dockAnchorFromCenter(
          payload.x + dockGeometry.width / 2,
          { x: dockGeometry.monitor.workArea.position.x, width: dockGeometry.monitor.workArea.size.width },
        ) });
      }
      // The undocking move itself can be claimed here before
      // handleDockedMove's async clear lands; persistDockAnchor re-reads
      // the pref after its awaits and drops that stale write.
      saveDockAnchor.schedule(payload.x, payload.y);
      return;
    }
    // Keep a pending undock correction at the live drop point while
    // retaining the existing trailing free-position persistence.
    if (freePosition) freePosition = { x: payload.x, y: payload.y };
    savePosition.schedule(payload.x, payload.y);
  };

  const runNativeMoveWorker = (session: NativeDragSession): Promise<void> => {
    if (session.moveWorker) return session.moveWorker;
    const worker = (async () => {
      // One monitor lookup may be in flight. While it is pending, native move
      // events overwrite this point; the next pass consumes only the newest
      // point, so a held drag cannot build an unbounded async backlog.
      while (ownsNativeDragSession(session)) {
        const payload = session.pendingPosition;
        session.pendingPosition = null;
        if (!payload) return;
        applyDraggedPosition(payload, session);
        await dockMovePending;
        if (!ownsNativeDragSession(session)) {
          session.pendingPosition = null;
          return;
        }
        // The worker consumes the coalescing slot before the monitor await;
        // retain the accepted point separately so terminal persistence sees
        // the actual last processed native position.
        session.lastAppliedPosition = { ...payload };
      }
      session.pendingPosition = null;
    })();
    session.moveWorker = worker;
    void worker.catch((error) => {
      console.warn("Floating window could not apply a native drag move", error);
    }).finally(() => {
      if (session.moveWorker !== worker) return;
      session.moveWorker = null;
      if (session.pendingPosition && ownsNativeDragSession(session)) {
        void runNativeMoveWorker(session);
      }
    });
    return worker;
  };

  const queueNativePosition = (session: NativeDragSession, payload: DockedMovePayload) => {
    session.pendingPosition = { ...payload };
    void runNativeMoveWorker(session);
  };

  const settleNativeMoveWorker = async (session: NativeDragSession): Promise<void> => {
    // At terminal there can be no further accepted moves. Repeat only to
    // bridge the worker-finally hand-off if an event arrived at that edge.
    while (ownsNativeDragSession(session)) {
      const worker = runNativeMoveWorker(session);
      await worker;
      if (!ownsNativeDragSession(session)) {
        session.pendingPosition = null;
        return;
      }
      if (!session.pendingPosition && !session.moveWorker) return;
    }
    session.pendingPosition = null;
  };

  const commitNativePosition = async (session: NativeDragSession) => {
    const payload = session.lastAppliedPosition;
    if (!payload || !ownsNativeDragSession(session)) return;
    const current = loadFloatingQuotaPrefs();
    if (current.dock !== "top") {
      if (freePosition) freePosition = { ...payload };
      saveFloatingQuotaPrefs({ ...current, x: payload.x, y: payload.y });
      return;
    }
    const geometry = dockGeometry;
    if (geometry) {
      saveFloatingQuotaPrefs({ ...current, dockAnchor: dockAnchorFromCenter(
        payload.x + geometry.width / 2,
        { x: geometry.monitor.workArea.position.x, width: geometry.monitor.workArea.size.width },
      ) });
      return;
    }
    const [monitor, size] = await Promise.all([
      currentMonitor().then((found) => found ?? primaryMonitor()),
      win.outerSize(),
    ]);
    if (!monitor || !ownsNativeDragSession(session)) return;
    const latest = loadFloatingQuotaPrefs();
    if (latest.dock !== "top") return;
    saveFloatingQuotaPrefs({ ...latest, dockAnchor: dockAnchorFromCenter(
      payload.x + size.width / 2,
      { x: monitor.workArea.position.x, width: monitor.workArea.size.width },
    ) });
  };

  const commitSmartDockRelease = async (session: NativeDragSession) => {
    const payload = session.lastAppliedPosition;
    if (!payload || !ownsNativeDragSession(session)) return;
    let recorded = session.smartDock;
    if (!recorded || recorded.x !== payload.x || recorded.y !== payload.y) {
      await evaluateNativeSmartDock(
        payload,
        session,
        dockGeometry?.width,
        () => ownsNativeDragSession(session),
      );
      recorded = session.smartDock;
    }
    if (!ownsNativeDragSession(session)) return;
    const phase = recorded && recorded.x === payload.x && recorded.y === payload.y
      ? recorded.phase
      : null;
    const current = loadFloatingQuotaPrefs();
    if (phase?.kind === "snap-preview" && phase.eligible && current.dock !== "top") {
      placementRevision++;
      session.revision = placementRevision;
      deferredResize = null;
      saveFloatingQuotaPrefs({ ...current, dock: "top", dockAnchor: phase.anchor });
      onDockChange?.("top");
      return;
    }
    if (phase?.kind === "unsnap-preview" && current.dock === "top") {
      placementRevision++;
      session.revision = placementRevision;
      deferredResize = null;
      parked = null;
      freePosition = { x: payload.x, y: payload.y };
      saveFloatingQuotaPrefs({ ...current, dock: "none", x: payload.x, y: payload.y });
      onDockChange?.("none");
      return;
    }
    if (phase?.kind === "docked" && current.dock === "top") {
      saveFloatingQuotaPrefs({ ...current, dockAnchor: phase.anchor });
      onDockChange?.("top");
      return;
    }
    await commitNativePosition(session);
  };

  const finishNativeDrag = (
    session: NativeDragSession,
    event: FloatingDragLifecycleEvent,
  ) => {
    if (!session.streamActive) return;
    if (event.reason !== "release") {
      cancelNativeDrag(session);
      return;
    }
    // End carries the authoritative final physical position. It is processed
    // as the final point, never as a synthetic begin/move callback.
    queueNativePosition(session, event.position);
    void settleNativeMoveWorker(session).then(async () => {
      if (ownsNativeDragSession(session)) await commitSmartDockRelease(session);
      concludeNativeDrag(session);
    }).catch((error) => {
      console.warn("Floating window could not finish a native drag", error);
      concludeNativeDrag(session);
    });
  };

  const dragLifecycle = createFloatingDragLifecycleConsumer({
    onBegin: (event) => {
      // A lifecycle begin is the single source of user-drag ownership. It
      // invalidates queued placement and drops any pre-gesture trailing save.
      savePosition.cancel();
      saveDockAnchor.cancel();
      placementRevision++;
      const current = loadFloatingQuotaPrefs();
      const session: NativeDragSession = {
        generation: event.generation,
        gesture: event.gesture,
        revision: placementRevision,
        ownsPlacement: true,
        streamActive: true,
        origin: {
          dock: current.dock,
          dockAnchor: current.dockAnchor,
          x: current.x,
          y: current.y,
          position: { ...event.position },
        },
        pendingPosition: null,
        lastAppliedPosition: null,
        moveWorker: null,
        smartDock: null,
      };
      nativeDragSession = session;
    },
    onMove: (event) => {
      const session = nativeDragSession;
      if (!session?.streamActive) return;
      queueNativePosition(session, event.position);
    },
    onEnd: (event) => {
      const session = nativeDragSession;
      if (!session?.streamActive) return;
      finishNativeDrag(session, event);
    },
    onCancel: () => {
      const session = nativeDragSession;
      if (session) cancelNativeDrag(session);
    },
  });

  cleanup = () => {
    if (disposed) return;
    disposed = true;
    if (gesturePreviewListener === (onGesturePreview ?? null)) {
      publishGesturePreview(null);
      gesturePreviewListener = null;
    }
    signal?.removeEventListener("abort", abort);
    const activeGesture = nativeDragSession?.streamActive;
    dragLifecycle.cancel();
    if (activeGesture && nativeDragSession?.streamActive) cancelNativeDrag(nativeDragSession);
    if (activeGesture) {
      savePosition.cancel();
      saveDockAnchor.cancel();
    } else {
      savePosition.flush();
      saveDockAnchor.flush();
    }
    for (const unlisten of unlistens) unlisten();
    if (lifecycleGeneration > 0) {
      void invoke("detach_floating_drag_lifecycle", { generation: lifecycleGeneration })
        .catch((error) => console.warn("Floating window could not detach native drag lifecycle", error));
      lifecycleGeneration = 0;
    }
  };

  if (abortRequested) {
    cleanup();
    return cleanup;
  }

  try {
    // Subscribe before attach so a quick begin/end emitted by the UI-thread
    // hook is buffered until its native generation becomes known.
    const unlistenLifecycle = await listen(FLOATING_DRAG_LIFECYCLE_EVENT, ({ payload }) => {
      dragLifecycle.accept(payload);
    });
    if (disposed) {
      unlistenLifecycle();
      return cleanup;
    }
    unlistens.push(unlistenLifecycle);
    const generation = await invoke<number>("attach_floating_drag_lifecycle");
    if (disposed) {
      if (Number.isSafeInteger(generation) && generation > 0) {
        void invoke("detach_floating_drag_lifecycle", { generation })
          .catch((error) => console.warn("Floating window could not detach native drag lifecycle", error));
      }
      return cleanup;
    }
    if (Number.isSafeInteger(generation) && generation > 0) {
      lifecycleGeneration = generation;
      dragLifecycle.activate(generation);
    } else if (generation === 0) {
      legacyMovedPersistence = true;
      dragLifecycle.activate(0);
    } else {
      dragLifecycle.cancel();
      console.error(
        "Floating window native drag lifecycle adapter returned an invalid generation; user drag persistence is disabled",
      );
    }
  } catch (error) {
    dragLifecycle.cancel();
    if (!disposed) {
      console.error(
        "Floating window native drag lifecycle adapter failed; user drag persistence is disabled",
        error,
      );
    }
  }

  if (disposed) return cleanup;

  // Showing the window can expose an immediate pointer drag. It follows
  // listener registration and native attachment so that first gesture cannot
  // race an unattached lifecycle consumer.
  if (prefs.visible) {
    try {
      await win.show();
    } catch (error) {
      console.warn("Floating window could not show itself", error);
    }
  }
  await announceFloatingVisibility(prefs.visible);
  if (disposed) return cleanup;

  try {
    unlistens.push(
      await win.onMoved(({ payload }) => {
        // Always consume an echo we requested. Once native lifecycle has a
        // generation, Tao geometry is observational only: it can arrive late
        // after a terminal or be caused by arbitrary SetWindowPos work.
        if (isRequestedMove(payload.x, payload.y) || lifecycleGeneration > 0 ||
          !legacyMovedPersistence || nativeDragSession?.streamActive || overlayOpen()) {
          return;
        }
        applyDraggedPosition(payload);
      }),
    );
    unlistens.push(await win.onFocusChanged(({ payload }) => {
      if (!payload) onBlur?.();
    }));
    unlistens.push(
      await win.onCloseRequested(() => {
        savePosition.flush();
        const current = loadFloatingQuotaPrefs();
        saveFloatingQuotaPrefs({ ...current, visible: false });
        void announceFloatingVisibility(false);
      }),
    );
    unlistens.push(
      await listen("tray://show-floating", () => {
        const current = loadFloatingQuotaPrefs();
        saveFloatingQuotaPrefs({ ...current, visible: true });
        void announceFloatingVisibility(true);
      }),
    );
    unlistens.push(
      await listen("tray://hide-floating", () => {
        const current = loadFloatingQuotaPrefs();
        saveFloatingQuotaPrefs({ ...current, visible: false });
        void announceFloatingVisibility(false);
      }),
    );
  } catch (error) {
    console.warn("Floating window could not subscribe to window events", error);
  }

  return cleanup;
}

/**
 * Resizes the bar window. While an overlay is open the grown bounds are
 * clamped into the work area on every call — the overlay height follows the
 * rendered card, so switching providers can grow the window again while it
 * stays open — and the pre-overlay position is parked so it can be put back
 * if the user has not dragged the window in the meantime.
 *
 * While docked the vertical position is not the user's: the window is
 * anchored to the top edge of its monitor's work area on every resize —
 * resting 600×16 or the full revealed bar — while the horizontal center
 * follows the persisted work-area-relative anchor (centered when unset) and
 * the whole window is clamped into the work area as one unit when the
 * monitor cannot host it.
 * The native size change is a single call, never animated; the reveal motion
 * is content-level (floating.css).
 */
export async function resizeFloatingWindow(
  width: number,
  height: number,
  overlay: boolean,
  docked = false,
): Promise<void> {
  if (!isTauriRuntime()) return;
  if (nativeDragSession?.streamActive) {
    // Monitor/reveal size changes must not reposition a window under native
    // pointer capture. Keep only the latest React intent for terminal apply.
    deferredResize = { width, height, overlay, docked, revision: placementRevision };
    return;
  }
  const revision = placementRevision;
  const run = resizeQueue.then(() => applyFloatingWindowSize(width, height, overlay, docked, revision));
  resizeQueue = run.catch(() => {});
  await run;
}

async function applyFloatingWindowSize(width: number, height: number, overlay: boolean, docked: boolean, revision: number): Promise<void> {
  const isCurrent = () => ownsPlacement(revision, docked);
  if (!isCurrent()) return;
  const win = getCurrentWindow();
  try {
    if (docked && !dockOrigin) {
      // First-run free coordinates may not have been persisted yet. Keep
      // their native origin in memory for an explicit Undock reconciliation.
      const origin = await win.outerPosition();
      if (!isCurrent()) return;
      dockOrigin = { x: origin.x, y: origin.y };
    }
    // Constraints must finish before size: independent React effects race
    // over IPC, leaving the 16px dock clamped to the free bar's 64px floor.
    await setFloatingDockConstraints(docked);
    if (!isCurrent()) return;
    if (docked) {
      parked = null;
      // Resolve a drag's live monitor before applying its anchor. This also
      // prevents an immediate collapse on another monitor using the old
      // monitor's temporary fraction.
      let pending: Promise<void>;
      do {
        pending = dockMovePending;
        await pending;
        if (!isCurrent()) return;
      } while (pending !== dockMovePending);
      const monitor = (await currentMonitor()) ?? (await primaryMonitor());
      if (!isCurrent()) return;
      if (!monitor) {
        await win.setSize(new LogicalSize(width, height));
        return;
      }
      const scale = monitor.scaleFactor || 1;
      const targetWidth = Math.round(width * scale);
      const targetHeight = Math.round(height * scale);
      const workArea = {
        x: monitor.workArea.position.x,
        y: monitor.workArea.position.y,
        width: monitor.workArea.size.width,
        height: monitor.workArea.size.height,
      };
      // The persisted anchor (work-area-relative fraction; null = centered)
      // places the resting strip and the revealed bar on one shared center so
      // the reveal never slides horizontally. widestHalf comes from the
      // resting width — the widest docked placement — so the clamp range
      // keeps every docked size inside the work area before the
      // per-placement clamp runs.
      const anchor = loadFloatingQuotaPrefs().dockAnchor ?? 0;
      const widestHalf = (Math.max(DOCK_WIDTH, width) * scale) / 2;
      const center = dockCenterForAnchor(anchor, workArea, widestHalf);
      const clamped = clampWindowBoundsToWorkArea(
        {
          x: Math.round(center - targetWidth / 2),
          y: workArea.y,
          width: targetWidth,
          height: targetHeight,
        },
        workArea,
      );
      await win.setSize(
        new PhysicalSize(clamped.bounds.width, clamped.bounds.height),
      );
      if (!isCurrent()) return;
      dockGeometry = { width: clamped.bounds.width, monitor };
      await positionFloatingWindow(clamped.bounds.x, clamped.bounds.y);
      return;
    }
    // Reconcile the committed free point even if a formerly owned native
    // move was already submitted when Undock/drag-away changed the intent.
    while (freePosition) {
      const position = await win.outerPosition();
      if (!isCurrent()) return;
      const restore = freePosition;
      if (position.x !== restore.x || position.y !== restore.y) {
        await positionFloatingWindow(restore.x, restore.y);
        if (!isCurrent()) return;
      }
      // A continued drag may have committed a newer drop point while this
      // native correction was in flight. Only consume the point we applied.
      if (freePosition === restore) freePosition = null;
    }
    if (!overlay && parked) {
      const position = await win.outerPosition();
      if (!isCurrent()) return;
      const restore = parked;
      parked = null;
      if (position.x === restore.shiftedX && position.y === restore.shiftedY) {
        await positionFloatingWindow(restore.x, restore.y);
        if (!isCurrent()) return;
      }
    }
    if (overlay) {
      const [position, monitor] = await Promise.all([
        win.outerPosition(),
        currentMonitor(),
      ]);
      if (!isCurrent()) return;
      if (monitor) {
        const scale = monitor.scaleFactor || 1;
        const targetWidth = Math.round(width * scale);
        const targetHeight = Math.round(height * scale);
        // A11: contain the shifted overlay on both axes (the original rule
        // only guarded the bottom). The 8px bottom breathing gap is kept.
        const clamped = clampWindowBoundsToWorkArea(
          { x: position.x, y: position.y, width: targetWidth, height: targetHeight },
          {
            x: monitor.workArea.position.x,
            y: monitor.workArea.position.y,
            width: monitor.workArea.size.width,
            height: Math.max(0, monitor.workArea.size.height - 8),
          },
        );
        if (clamped.changed && !parked) {
          parked = {
            x: position.x,
            y: position.y,
            shiftedX: clamped.bounds.x,
            shiftedY: clamped.bounds.y,
          };
        }
        if (clamped.changed) {
          await positionFloatingWindow(clamped.bounds.x, clamped.bounds.y);
          if (!isCurrent()) return;
        }
      }
    }
    await win.setSize(new LogicalSize(width, height));
  } catch (error) {
    console.warn("Floating window could not resize", error);
  }
}

/**
 * Smallest dock re-anchor hardening: while docked, the floating window
 * re-derives its top-center anchor when the DPI scale changes or the window
 * regains focus (the same observable proxies the main window reclamps on —
 * they catch a scale-factor change, a monitor change, and a work-area change
 * that moved the top edge). The callback just nudges the floating window's
 * resize effect, which recomputes the anchor from the live monitor state.
 * Returns a cleanup that drops the listeners. No-op outside Tauri.
 */
export function attachDockReanchor(onReanchor: () => void): () => void {
  if (!isTauriRuntime()) return () => {};
  const win = getCurrentWindow();
  const unlistens: Promise<() => void>[] = [];
  let active = true;
  const isDocked = () => active && !nativeDragSession?.streamActive &&
    loadFloatingQuotaPrefs().dock === "top";
  try {
    unlistens.push(win.onScaleChanged(() => { if (isDocked()) onReanchor(); }));
    unlistens.push(
      win.onFocusChanged(({ payload: focused }) => {
        if (!focused || !isDocked()) return;
        const revision = placementRevision;
        // A normal click to drag focuses the window. Reposition only if the
        // monitor/work area actually changed, never fight that native drag.
        void currentMonitor().then((monitor) => {
          if (!active || nativeDragSession?.streamActive || !ownsPlacement(revision, true)) return;
          const prior = dockGeometry?.monitor;
          if (!monitor || !prior || monitor.scaleFactor !== prior.scaleFactor ||
            monitor.workArea.position.x !== prior.workArea.position.x ||
            monitor.workArea.position.y !== prior.workArea.position.y ||
            monitor.workArea.size.width !== prior.workArea.size.width ||
            monitor.workArea.size.height !== prior.workArea.size.height) onReanchor();
        }).catch(() => {});
      }),
    );
  } catch (error) {
    console.warn("Floating window could not subscribe to re-anchor events", error);
  }
  return () => {
    active = false;
    for (const unlisten of unlistens) {
      unlisten.then((fn) => fn()).catch(() => {});
    }
  };
}
