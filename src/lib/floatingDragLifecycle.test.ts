import { describe, expect, it, vi } from "vitest";
import {
  createFloatingDragLifecycleConsumer,
  type FloatingDragLifecycleEvent,
} from "./floatingDragLifecycle";

function event(
  phase: FloatingDragLifecycleEvent["phase"],
  sequence: number,
  overrides: Partial<FloatingDragLifecycleEvent> = {},
): FloatingDragLifecycleEvent {
  return {
    generation: 7,
    gesture: 3,
    sequence,
    phase,
    reason: phase === "end" ? "release" : null,
    position: { x: 40, y: 50 },
    ...overrides,
  };
}

function consumer() {
  const callbacks = {
    onBegin: vi.fn(),
    onMove: vi.fn(),
    onEnd: vi.fn(),
    onCancel: vi.fn(),
  };
  return { callbacks, consumer: createFloatingDragLifecycleConsumer(callbacks) };
}

describe("floating drag lifecycle consumer", () => {
  it("buffers a fast native begin/end until attach returns without inventing a move", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("end", 2));
    lifecycle.activate(7);

    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onMove).not.toHaveBeenCalled();
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
    expect(lifecycle.active).toBe(false);
  });

  it("holds an open gesture across a pause and accepts its later terminal", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("move", 2));

    expect(lifecycle.active).toBe(true);
    expect(callbacks.onEnd).not.toHaveBeenCalled();

    lifecycle.accept(event("end", 3));
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
  });

  it("drops an overflowed startup stream and waits for a new live begin", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.accept(event("begin", 1));
    for (let sequence = 2; sequence <= 33; sequence++) {
      lifecycle.accept(event("move", sequence));
    }
    lifecycle.accept(event("end", 34));
    lifecycle.activate(7);

    expect(callbacks.onBegin).not.toHaveBeenCalled();
    expect(callbacks.onMove).not.toHaveBeenCalled();
    expect(callbacks.onEnd).not.toHaveBeenCalled();
    expect(lifecycle.active).toBe(false);

    lifecycle.accept(event("begin", 1, { gesture: 4 }));
    lifecycle.accept(event("end", 2, { gesture: 4 }));
    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
  });

  it("rejects malformed, stale, duplicate, out-of-order, and move-without-begin traffic", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("move", 1));
    lifecycle.accept({ ...event("begin", 1), position: { x: Number.NaN, y: 1 } });
    lifecycle.accept(event("begin", 2));
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("move", 1));
    lifecycle.accept(event("move", 2, { generation: 6 }));
    lifecycle.accept(event("move", 2));
    lifecycle.accept(event("move", 2));
    lifecycle.accept(event("end", 3, { reason: "cancel" }));
    lifecycle.accept(event("end", 4, { reason: "unknown" }));

    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onMove).toHaveBeenCalledOnce();
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
  });

  it("accepts only the attached generation and never lets cleanup revive a late callback", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.accept(event("begin", 1, { generation: 6 }));
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1, { generation: 6 }));
    lifecycle.accept(event("begin", 1));
    lifecycle.cancel();
    lifecycle.accept(event("move", 2));
    lifecycle.accept(event("end", 3));
    lifecycle.cancel();

    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onCancel).toHaveBeenCalledOnce();
    expect(callbacks.onMove).not.toHaveBeenCalled();
    expect(callbacks.onEnd).not.toHaveBeenCalled();
  });

  it("a stale terminal from an older gesture cannot close or leak into a newer one", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("end", 2, { reason: "cancel" }));
    lifecycle.accept(event("begin", 1, { gesture: 4 }));
    // Duplicate and stale terminal evidence for the finished gesture.
    lifecycle.accept(event("end", 9, { gesture: 3, reason: "release" }));
    lifecycle.accept(event("move", 10, { gesture: 3 }));
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
    expect(lifecycle.active).toBe(true);
    // The newer gesture still terminates exactly once.
    lifecycle.accept(event("end", 2, { gesture: 4, reason: "release" }));
    expect(callbacks.onEnd).toHaveBeenCalledTimes(2);
    expect(callbacks.onEnd).toHaveBeenLastCalledWith(
      expect.objectContaining({ gesture: 4, reason: "release" }),
    );
  });

  it("a terminal that does not advance the sequence is rejected until the real one lands", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("move", 2));
    lifecycle.accept(event("end", 2, { reason: "release" }));
    expect(callbacks.onEnd).not.toHaveBeenCalled();
    expect(lifecycle.active).toBe(true);
    lifecycle.accept(event("end", 3, { reason: "release" }));
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
    expect(lifecycle.active).toBe(false);
  });

  it("activating a newer generation supersedes an active session and its late traffic", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1));
    lifecycle.accept(event("move", 2));
    lifecycle.activate(9); // a newer native attach supersedes the open gesture
    expect(lifecycle.active).toBe(false);
    lifecycle.accept(event("end", 3)); // generation-7 traffic is stale now
    expect(callbacks.onEnd).not.toHaveBeenCalled();
    // Gesture numbering restarts under the new generation.
    lifecycle.accept(event("begin", 1, { generation: 9, gesture: 1 }));
    lifecycle.accept(event("end", 2, { generation: 9, gesture: 1, reason: "release" }));
    expect(callbacks.onBegin).toHaveBeenCalledTimes(2);
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
  });

  it("a rapid begin→move→release forwards each position exactly once, in order", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept(event("begin", 1, { position: { x: 10, y: 20 } }));
    lifecycle.accept(event("move", 2, { position: { x: 30, y: 40 } }));
    lifecycle.accept(event("move", 3, { position: { x: 50, y: 60 } }));
    lifecycle.accept(event("end", 4, { position: { x: 70, y: 80 } }));
    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onMove).toHaveBeenCalledTimes(2);
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
    expect(callbacks.onMove.mock.calls.map(([moved]) => moved.position)).toEqual([
      { x: 30, y: 40 },
      { x: 50, y: 60 },
    ]);
    expect(callbacks.onEnd.mock.calls[0][0].position).toEqual({ x: 70, y: 80 });
  });

  it("only a real terminal may carry a reason, and only a known one", () => {
    const { callbacks, consumer: lifecycle } = consumer();
    lifecycle.activate(7);
    lifecycle.accept({ ...event("begin", 1), reason: "release" });
    lifecycle.accept(event("begin", 1));
    lifecycle.accept({ ...event("move", 2), reason: "cancel" });
    lifecycle.accept(event("move", 2));
    lifecycle.accept({ ...event("end", 3), reason: "explode" as unknown as "release" });
    expect(callbacks.onBegin).toHaveBeenCalledOnce();
    expect(callbacks.onMove).toHaveBeenCalledOnce();
    expect(callbacks.onEnd).not.toHaveBeenCalled();
    expect(lifecycle.active).toBe(true);
    lifecycle.accept(event("end", 3));
    expect(callbacks.onEnd).toHaveBeenCalledOnce();
  });
});
