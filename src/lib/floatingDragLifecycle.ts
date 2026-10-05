/** Native event name for the bounded floating-window drag lifecycle. */
export const FLOATING_DRAG_LIFECYCLE_EVENT = "floating://drag-lifecycle";

export type FloatingDragPhase = "begin" | "move" | "end";
export type FloatingDragEndReason = "release" | "cancel" | "unknown";

export type FloatingDragLifecycleEvent = {
  generation: number;
  gesture: number;
  sequence: number;
  phase: FloatingDragPhase;
  reason: FloatingDragEndReason | null;
  position: { x: number; y: number };
};

const STARTUP_BUFFER_LIMIT = 32;

type FloatingDragLifecycleCallbacks = {
  onBegin: (event: FloatingDragLifecycleEvent) => void;
  onMove: (event: FloatingDragLifecycleEvent) => void;
  onEnd: (event: FloatingDragLifecycleEvent) => void;
  /** Local teardown has no native terminal event, but must cancel ownership. */
  onCancel: () => void;
};

/**
 * Validates and orders one native subscription's drag events. Listening must
 * start before attach resolves, so a small startup buffer retains a confirmed
 * begin/end pair that races the command response. No event is invented:
 * especially, begin never becomes an onMove callback.
 */
export function createFloatingDragLifecycleConsumer(
  callbacks: FloatingDragLifecycleCallbacks,
) {
  let generation: number | null = null;
  let accepting = true;
  let active: { gesture: number; sequence: number } | null = null;
  let lastGesture = 0;
  const startup: unknown[] = [];
  let startupOverflowed = false;

  const accept = (value: unknown) => {
    if (!accepting) return;
    if (generation === null) {
      // A bounded buffer protects the attach race without retaining arbitrary
      // event traffic if a command never resolves. Overflow must not leave a
      // retained begin without its terminal: discard the whole pre-attach
      // stream and wait for a new live begin after activation.
      if (startupOverflowed) return;
      if (startup.length >= STARTUP_BUFFER_LIMIT) {
        startup.length = 0;
        startupOverflowed = true;
        return;
      }
      startup.push(value);
      return;
    }
    const event = parseFloatingDragLifecycleEvent(value);
    if (!event || event.generation !== generation) return;

    if (event.phase === "begin") {
      if (active || event.sequence !== 1 || event.gesture <= lastGesture) return;
      active = { gesture: event.gesture, sequence: event.sequence };
      callbacks.onBegin(event);
      return;
    }

    if (!active || active.gesture !== event.gesture || event.sequence <= active.sequence) {
      return;
    }
    active.sequence = event.sequence;
    if (event.phase === "move") {
      callbacks.onMove(event);
      return;
    }

    // Only an accepted native terminal closes the gesture. Its reason is
    // checked by parseFloatingDragLifecycleEvent.
    lastGesture = event.gesture;
    active = null;
    callbacks.onEnd(event);
  };

  return {
    accept,
    activate(nextGeneration: number) {
      if (!Number.isSafeInteger(nextGeneration) || nextGeneration <= 0 || !accepting) {
        startup.length = 0;
        startupOverflowed = false;
        return;
      }
      generation = nextGeneration;
      active = null;
      lastGesture = 0;
      if (startupOverflowed) {
        startup.length = 0;
        startupOverflowed = false;
        return;
      }
      const buffered = startup.splice(0);
      for (const value of buffered) accept(value);
    },
    /** Idempotent local terminal for unmount or a failed/late attach. */
    cancel() {
      if (!accepting) return;
      accepting = false;
      startup.length = 0;
      startupOverflowed = false;
      if (active) {
        active = null;
        callbacks.onCancel();
      }
    },
    get active() {
      return active !== null;
    },
  };
}

function parseFloatingDragLifecycleEvent(value: unknown): FloatingDragLifecycleEvent | null {
  if (!value || typeof value !== "object") return null;
  const event = value as Record<string, unknown>;
  const position = event.position;
  if (!position || typeof position !== "object") return null;
  const point = position as Record<string, unknown>;
  if (
    !isPositiveInteger(event.generation) ||
    !isPositiveInteger(event.gesture) ||
    !isPositiveInteger(event.sequence) ||
    !Number.isFinite(point.x) ||
    !Number.isFinite(point.y) ||
    (event.phase !== "begin" && event.phase !== "move" && event.phase !== "end")
  ) {
    return null;
  }
  if (event.phase === "end") {
    if (event.reason !== "release" && event.reason !== "cancel" && event.reason !== "unknown") {
      return null;
    }
  } else if (event.reason !== null) {
    return null;
  }
  return {
    generation: event.generation,
    gesture: event.gesture,
    sequence: event.sequence,
    phase: event.phase,
    reason: event.reason,
    position: { x: point.x as number, y: point.y as number },
  };
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
