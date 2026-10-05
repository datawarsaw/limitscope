import { describe, expect, it } from "vitest";
import { buildSegments, predictWindow, predictWindows } from "./engine";
import type { QuotaObservation } from "./types";
import { CRITICAL_PERCENT, NEAR_LIMIT_PERCENT } from "../thresholds";

// Fixed reference timeline: 2026-09-27T06:00:00Z is minute 0.
const BASE = Date.parse("2026-09-27T06:00:00Z");
const iso = (minutes: number): string =>
  new Date(BASE + minutes * 60_000).toISOString();

/** "now" for most cases: six hours of history, sampled every 15 minutes. */
const NOW = iso(360);

const HOUR_MS = 3_600_000;

function obsAt(
  minutes: number,
  usedPercent: number,
  resetAt?: string,
): QuotaObservation {
  return {
    providerId: "p",
    windowLabel: "w",
    usedPercent,
    observedAt: iso(minutes),
    ...(resetAt !== undefined ? { resetAt } : {}),
  };
}

function series(
  from: number,
  to: number,
  step: number,
  percentAt: (minutes: number) => number,
  resetAt?: string,
): QuotaObservation[] {
  const out: QuotaObservation[] = [];
  for (let m = from; m <= to; m += step) out.push(obsAt(m, percentAt(m), resetAt));
  return out;
}

function predict(
  observations: QuotaObservation[],
  now: string = NOW,
  options?: Parameters<typeof predictWindow>[0]["options"],
) {
  return predictWindow({
    observations,
    providerId: "p",
    windowLabel: "w",
    now,
    ...(options !== undefined ? { options } : {}),
  });
}

/**
 * A projected crossing goes through float division, so compare its ISO time
 * against the exact minute with a 2 ms tolerance — tight enough to pin the
 * math, loose enough to ignore float noise below one millisecond.
 */
function expectCrossingAt(
  estimatedAt: string | undefined,
  minute: number,
): void {
  expect(estimatedAt).toBeDefined();
  expect(
    Math.abs(Date.parse(estimatedAt!) - (BASE + minute * 60_000)),
  ).toBeLessThan(2);
}

describe("predictWindow — steady usage", () => {
  const resetAt = iso(480); // reset two hours after NOW
  const steady = series(0, 360, 15, (m) => 10 + 4 * (m / 60), resetAt);

  it("fits a linear burn rate, projects it to the reset, and reports high confidence", () => {
    const p = predict(steady)!;
    expect(p.burnRatePerHour).toBeCloseTo(4, 6);
    expect(p.projectedPercentAtReset).toBeCloseTo(42, 6);
    expect(p.willExhaustBeforeReset).toBe(false);
    expect(p.confidence).toBe("high");
    expect(p.risk).toBe("low");
    expect(p.basis.segmentCount).toBe(1);
    expect(p.basis.segmentSampleCount).toBe(25);
    expect(p.basis.fitSampleCount).toBe(25);
    expect(p.basis.fitSpanMinutes).toBe(360);
    expect(p.basis.fitMeanGapMinutes).toBe(15);
    expect(p.basis.usedWholeSegment).toBe(false);
    expect(p.basis.isStale).toBe(false);
  });

  it("anchors exhaustion on the newest observation (66 points at 4/h = 16.5h)", () => {
    const p = predict(steady)!;
    expect(p.estimatedExhaustionAt).toBe("2026-09-28T04:30:00.000Z");
  });
});

describe("predictWindow — accelerating usage", () => {
  it("reports a burn rate between the initial and final instantaneous rates", () => {
    // percent = 2t + 1.5t^2, t in hours: 2/h at the start, 20/h at hour six.
    const accelerating = series(
      0,
      360,
      15,
      (m) => 2 * (m / 60) + 1.5 * (m / 60) ** 2,
      iso(720),
    );
    const p = predict(accelerating)!;
    expect(p.burnRatePerHour).toBeGreaterThan(2);
    expect(p.burnRatePerHour).toBeLessThan(20);
    // Least squares over the whole span lands on (a + b*(t0+t1)/... ) = 11/h.
    expect(p.burnRatePerHour).toBeCloseTo(11, 3);
    expect(p.willExhaustBeforeReset).toBe(true);
    expect(p.risk).toBe("high");
  });
});

describe("predictWindow — idle window", () => {
  it("reports zero burn, no exhaustion time, and low risk", () => {
    const idle = series(0, 360, 15, () => 42, iso(480));
    const p = predict(idle)!;
    expect(p.burnRatePerHour).toBe(0);
    expect(p.projectedPercentAtReset).toBeCloseTo(42, 6);
    expect(p.estimatedExhaustionAt).toBeUndefined();
    expect(p.willExhaustBeforeReset).toBe(false);
    expect(p.confidence).toBe("high");
    expect(p.risk).toBe("low");
  });

  it("marks a window sitting exactly at the 80% risk threshold as medium", () => {
    const p = predict(series(0, 360, 15, () => 80, iso(480)))!;
    expect(p.projectedPercentAtReset).toBe(80);
    expect(p.willExhaustBeforeReset).toBe(false);
    expect(p.risk).toBe("medium");
  });
});

describe("predictWindow — resets", () => {
  it("starts a new segment on a percentage drop and never blends cycles", () => {
    // 20%/h before the reset at minute 180, 6%/h afterwards.
    const pre = series(0, 180, 15, (m) => 10 + 20 * (m / 60), iso(200));
    const post = series(195, 360, 15, (m) => 2 + 6 * ((m - 180) / 60), iso(660));
    const p = predict([...pre, ...post])!;
    expect(p.basis.segmentCount).toBe(2);
    expect(p.basis.segmentSampleCount).toBe(12);
    expect(p.burnRatePerHour).toBeCloseTo(6, 6);
    expect(p.burnRatePerHour).toBeLessThan(10);
    expect(p.projectedPercentAtReset).toBeCloseTo(50, 6);
    expect(p.confidence).toBe("medium");
    expect(p.risk).toBe("low");
    expect(p.basis.resetAt).toBe(iso(660));
  });

  it("splits on a drop even when resetAt never changes", () => {
    const pre = series(0, 180, 15, (m) => 10 + 20 * (m / 60), iso(660));
    const post = series(195, 360, 15, (m) => 2 + 6 * ((m - 180) / 60), iso(660));
    const segments = buildSegments([...pre, ...post]);
    expect(segments).toHaveLength(2);
    expect(segments[0].openedByReset).toBe(false);
    expect(segments[1].openedByReset).toBe(true);
    const p = predict([...pre, ...post])!;
    expect(p.basis.segmentCount).toBe(2);
    expect(p.burnRatePerHour).toBeCloseTo(6, 6);
  });

  it("splits on a drop even when resetAt is missing entirely", () => {
    const pre = series(0, 180, 15, (m) => 10 + 20 * (m / 60));
    const post = series(195, 360, 15, (m) => 2 + 6 * ((m - 180) / 60));
    const p = predict([...pre, ...post])!;
    expect(p.basis.segmentCount).toBe(2);
    expect(p.burnRatePerHour).toBeCloseTo(6, 6);
    expect(p.projectedPercentAtReset).toBeUndefined();
    expect(p.confidence).toBe("low");
  });

  it("splits when resetAt moves forward, even without a visible drop", () => {
    const firstCycle = series(0, 150, 30, (m) => 10 + 5 * (m / 60), iso(480));
    const secondCycle = series(180, 360, 30, (m) => 22.5 + 5 * ((m - 180) / 60), iso(960));
    const p = predict([...firstCycle, ...secondCycle])!;
    expect(p.basis.segmentCount).toBe(2);
    expect(p.basis.resetAt).toBe(iso(960));
    expect(p.burnRatePerHour).toBeCloseTo(5, 6);
  });

  it("ignores resetAt jitter inside the tolerance and keeps the newest value", () => {
    const jittered = series(0, 360, 30, (m) => 10 + 4 * (m / 60)).map((o, i) =>
      i < 10
        ? { ...o, resetAt: iso(480) }
        : { ...o, resetAt: new Date(BASE + 480 * 60_000 + 30_000).toISOString() },
    );
    expect(buildSegments(jittered)).toHaveLength(1);
    const p = predict(jittered)!;
    expect(p.basis.segmentCount).toBe(1);
    expect(p.basis.resetAt).toBe(new Date(BASE + 480 * 60_000 + 30_000).toISOString());
    expect(p.basis.resetExpired).toBe(false);
  });

  it("treats a backward resetAt as a correction, not a new cycle", () => {
    const drifted = series(0, 360, 30, (m) => 10 + 4 * (m / 60)).map((o, i) => ({
      ...o,
      resetAt: i <= 6 ? iso(480) : iso(300), // regressed, and already in the past
    }));
    expect(buildSegments(drifted)).toHaveLength(1);
    const p = predict(drifted)!;
    expect(p.basis.resetAt).toBe(iso(300));
    expect(p.basis.resetExpired).toBe(true);
    expect(p.projectedPercentAtReset).toBeUndefined();
    expect(p.willExhaustBeforeReset).toBeUndefined();
    expect(p.confidence).toBe("low");
    expect(p.risk).toBeUndefined();
  });
});

describe("predictWindow — projection edges", () => {
  it("flags exhaustion before a near reset and clamps the projection at 100%", () => {
    const rising = series(0, 360, 15, (m) => 20 + 12 * (m / 60), iso(480));
    const p = predict(rising)!;
    expect(p.burnRatePerHour).toBeCloseTo(12, 6);
    expect(p.projectedPercentAtReset).toBe(100);
    expect(p.willExhaustBeforeReset).toBe(true);
    expect(p.risk).toBe("high");
    expect(
      Math.abs(Date.parse(p.estimatedExhaustionAt!) - (BASE + 400 * 60_000)),
    ).toBeLessThan(5);
  });

  it("reports exhaustion after the reset as medium risk when projected past 80%", () => {
    const rising = series(0, 360, 15, (m) => 40 + 4 * (m / 60), iso(660));
    const p = predict(rising)!;
    expect(p.projectedPercentAtReset).toBeCloseTo(84, 6);
    expect(p.willExhaustBeforeReset).toBe(false);
    expect(p.risk).toBe("medium");
    expect(p.confidence).toBe("high");
  });

  it("treats an already-spent window as exhausted now", () => {
    const spent = series(0, 360, 15, () => 100, iso(480));
    const p = predict(spent)!;
    expect(p.burnRatePerHour).toBe(0);
    expect(p.projectedPercentAtReset).toBe(100);
    expect(p.estimatedExhaustionAt).toBe(iso(360));
    expect(p.willExhaustBeforeReset).toBe(true);
    expect(p.risk).toBe("high");
    expect(p.confidence).toBe("high");
  });

  it("drops the projection fields when resetAt is missing but keeps the burn rate", () => {
    const p = predict(series(0, 360, 15, (m) => 10 + 4 * (m / 60)))!;
    expect(p.burnRatePerHour).toBeCloseTo(4, 6);
    expect(p.projectedPercentAtReset).toBeUndefined();
    expect(p.willExhaustBeforeReset).toBeUndefined();
    expect(p.risk).toBeUndefined();
    expect(p.confidence).toBe("low");
    expect(Date.parse(p.estimatedExhaustionAt!)).toBeGreaterThan(BASE);
  });
});

describe("predictWindow — sparse and stale history", () => {
  it("returns insufficient with no numbers for a single sample", () => {
    const p = predict([obsAt(360, 10, iso(480))])!;
    expect(p.confidence).toBe("insufficient");
    expect(p.burnRatePerHour).toBeUndefined();
    expect(p.projectedPercentAtReset).toBeUndefined();
    expect(p.estimatedExhaustionAt).toBeUndefined();
    expect(p.willExhaustBeforeReset).toBeUndefined();
    expect(p.risk).toBeUndefined();
    expect(p.basis.segmentSampleCount).toBe(1);
  });

  it("returns insufficient with an empty basis when there is no history", () => {
    const p = predict([])!;
    expect(p.confidence).toBe("insufficient");
    expect(p.basis.segmentId).toBe("none");
    expect(p.basis.segmentCount).toBe(0);
    expect(p.basis.fitSampleCount).toBe(0);
    expect(p.risk).toBeUndefined();
  });

  it("caps confidence at low for a source that has not been refreshed", () => {
    const steady = series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(720));
    const p = predict(steady, iso(540))!; // three hours after the last sample
    expect(p.basis.latestAgeMinutes).toBe(180);
    expect(p.basis.isStale).toBe(true);
    expect(p.burnRatePerHour).toBeCloseTo(4, 6);
    expect(p.confidence).toBe("low");
  });

  it("reaches only medium confidence for a two-sample, four-hour span", () => {
    const p = predict([obsAt(120, 20, iso(480)), obsAt(360, 40, iso(480))])!;
    expect(p.burnRatePerHour).toBeCloseTo(5, 6);
    expect(p.basis.fitSpanMinutes).toBe(240);
    expect(p.confidence).toBe("medium");
  });

  it("holds medium confidence across a long gap between samples", () => {
    const p = predict([obsAt(0, 10, iso(480)), obsAt(15, 11, iso(480)), obsAt(345, 45, iso(480))])!;
    expect(p.basis.fitSampleCount).toBe(3);
    expect(p.basis.fitSpanMinutes).toBe(345);
    expect(p.burnRatePerHour).toBeGreaterThan(0);
    expect(p.confidence).toBe("medium");
  });

  it("ignores samples older than the maximum age", () => {
    const p = predict([
      obsAt(-1500, 5, iso(480)),
      obsAt(-1460, 6, iso(480)),
      obsAt(345, 40, iso(480)),
      obsAt(360, 50, iso(480)),
    ])!;
    expect(p.basis.segmentSampleCount).toBe(2);
    expect(p.basis.fitSampleCount).toBe(2);
    expect(p.basis.fitSpanMinutes).toBe(15);
    expect(p.confidence).toBe("low");
  });
});

describe("predictWindow — noise and clock skew", () => {
  it("absorbs small backward jitter inside one segment", () => {
    const percents = [50, 52, 51, 53, 55, 54, 56];
    const noisy = percents.map((percent, i) => obsAt(i * 30, percent, iso(480)));
    expect(buildSegments(noisy)).toHaveLength(1);
    // Evaluate at the last sample: three hours later the source is stale.
    const p = predict(noisy, iso(180))!;
    expect(p.basis.segmentCount).toBe(1);
    expect(p.burnRatePerHour).toBeGreaterThan(0);
    expect(p.burnRatePerHour).toBeLessThan(3);
    expect(p.confidence).toBe("high");
  });

  it("drops future-dated samples beyond the skew tolerance and keeps the rest", () => {
    const steady = series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(480));
    const withSkew = [
      ...steady,
      obsAt(362, 34.2, iso(480)), // two minutes ahead: tolerated
      obsAt(370, 99, iso(480)), // ten minutes ahead: dropped
    ];
    const p = predict(withSkew)!;
    // The oldest steady sample falls outside the 6h window; the tolerated
    // 362-minute sample is kept and becomes the newest usable observation.
    expect(p.basis.fitSampleCount).toBe(25);
    expect(p.basis.lastObservedAt).toBe(iso(362));
    expect(p.basis.latestAgeMinutes).toBe(0);
    expect(p.basis.isStale).toBe(false);
    expect(p.confidence).toBe("high");
  });

  it("rejects non-finite percentages, broken timestamps, and blank labels", () => {
    const junk: QuotaObservation[] = [
      { providerId: "p", windowLabel: "w", usedPercent: Number.NaN, observedAt: iso(0) },
      { providerId: "p", windowLabel: "w", usedPercent: 10, observedAt: "not-a-time" },
      { providerId: "p", windowLabel: "", usedPercent: 10, observedAt: iso(0) },
    ];
    expect(buildSegments(junk)).toEqual([]);
    expect(predict(junk)!.confidence).toBe("insufficient");
  });

  it("keeps only the last sample for duplicate timestamps", () => {
    const segments = buildSegments([obsAt(0, 10), obsAt(0, 20), obsAt(15, 25)]);
    expect(segments).toHaveLength(1);
    expect(segments[0].samples.map((s) => s.usedPercent)).toEqual([20, 25]);
  });

  it("clamps out-of-range percentages instead of discarding the sample", () => {
    const segments = buildSegments([obsAt(0, -5), obsAt(15, 140)]);
    expect(segments[0].samples.map((s) => s.usedPercent)).toEqual([0, 100]);
  });

  it("throws when the reference time is unparseable", () => {
    expect(() => predict(series(0, 60, 30, () => 10), "nope")).toThrow(TypeError);
  });
});

describe("history window selection", () => {
  // Twenty hours of 1%/h, then a two-hour burst at 10%/h.
  const bursty = series(-840, 360, 30, (m) => {
    const elapsedHours = (m + 840) / 60;
    return elapsedHours <= 16 ? 20 + elapsedHours : 36 + 10 * (elapsedHours - 16);
  }, iso(720));

  it("reacts to a recent burst within one hour", () => {
    const p = predict(bursty, NOW, { rateWindowMs: HOUR_MS })!;
    expect(p.burnRatePerHour).toBeCloseTo(10, 4);
    expect(p.basis.fitSampleCount).toBe(3);
    expect(p.basis.usedWholeSegment).toBe(false);
    expect(p.confidence).toBe("medium"); // dense enough in rate, too few samples for high
  });

  it("prices in more of the day at six hours", () => {
    const p = predict(bursty, NOW, { rateWindowMs: 6 * HOUR_MS })!;
    expect(p.basis.fitSampleCount).toBe(13);
    expect(p.basis.fitSpanMinutes).toBe(360);
    expect(p.burnRatePerHour).toBeGreaterThan(2);
    expect(p.burnRatePerHour).toBeLessThan(10);
    expect(p.confidence).toBe("high");
  });

  it("smooths the burst away over the full day", () => {
    const oneHour = predict(bursty, NOW, { rateWindowMs: HOUR_MS })!;
    const sixHours = predict(bursty, NOW, { rateWindowMs: 6 * HOUR_MS })!;
    const fullDay = predict(bursty, NOW, { rateWindowMs: 24 * HOUR_MS })!;
    expect(oneHour.burnRatePerHour).toBeGreaterThan(sixHours.burnRatePerHour!);
    expect(sixHours.burnRatePerHour).toBeGreaterThan(fullDay.burnRatePerHour!);
    expect(fullDay.basis.fitSpanMinutes).toBe(1200);
    expect(fullDay.confidence).toBe("high");
  });

  it("falls back to the whole segment when the recent window is too thin", () => {
    // Nothing inside the last six hours except the final sample, so the fit
    // widens to the whole segment instead of reporting no burn rate.
    const p = predict([obsAt(-600, 10, iso(480)), obsAt(-300, 25, iso(480)), obsAt(360, 45, iso(480))])!;
    expect(p.basis.fitSampleCount).toBe(3);
    expect(p.basis.usedWholeSegment).toBe(true);
    expect(p.burnRatePerHour).toBeGreaterThan(0);
  });
});

describe("predictWindow — projection anchor", () => {
  const resetAt = iso(480); // reset two hours after the last sample
  const steady = series(0, 360, 15, (m) => 10 + 4 * (m / 60), resetAt);

  it("keeps the projection and exhaustion time stable as now advances without a new sample", () => {
    const atSample = predict(steady, iso(360))!;
    const twentyFiveMinutesLater = predict(steady, iso(385))!;
    expect(atSample.projectedPercentAtReset).toBeCloseTo(42, 6);
    expect(twentyFiveMinutesLater.projectedPercentAtReset).toBeCloseTo(42, 6);
    expect(twentyFiveMinutesLater.estimatedExhaustionAt).toBe(
      atSample.estimatedExhaustionAt,
    );
    expect(twentyFiveMinutesLater.burnRatePerHour).toBeCloseTo(4, 6);
    // 25 minutes past the newest sample: still fresh, still anchored on it.
    expect(twentyFiveMinutesLater.basis.isStale).toBe(false);
    expect(twentyFiveMinutesLater.basis.lastObservedAt).toBe(iso(360));
  });

  it("anchors the projection and exhaustion on the newest observation, not now", () => {
    const p = predict(steady, iso(385))!;
    // 34% at the last sample plus 4%/h over the two hours to the reset.
    expect(p.projectedPercentAtReset).toBeCloseTo(42, 6);
    // (100 − 34)% at 4%/h lands 16.5h after the last sample.
    expect(p.estimatedExhaustionAt).toBe("2026-09-28T04:30:00.000Z");
  });

  it("suppresses an unconfirmed past exhaustion instead of warning about it", () => {
    // 12%/h into 97%: exhaustion predicted 15 minutes after the last sample
    // while the reset is still two hours out. Twenty-five minutes later no
    // confirming observation has arrived, so the claim must be withdrawn —
    // the UI renders the "Likely exhaustion" line only when
    // willExhaustBeforeReset is true, so this hides that line.
    const late = [obsAt(330, 91, resetAt), obsAt(345, 94, resetAt), obsAt(360, 97, resetAt)];
    const before = predict(late, iso(360))!;
    expect(before.estimatedExhaustionAt).toBe(iso(375));
    expect(before.willExhaustBeforeReset).toBe(true);
    const after = predict(late, iso(385))!;
    expect(after.estimatedExhaustionAt).toBe(iso(375)); // still anchored
    expect(after.willExhaustBeforeReset).toBeUndefined();
    expect(after.projectedPercentAtReset).toBe(100); // clamped, stable
    expect(after.risk).toBe("medium");
  });
});

describe("predictWindows", () => {
  it("estimates every provider/window pair present, sorted by identity", () => {
    const observations: QuotaObservation[] = [
      ...series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(480)).map((o) => ({
        ...o,
        providerId: "p1",
      })),
      ...[0, 180, 360].map((m) => ({
        providerId: "p1",
        windowLabel: "other",
        usedPercent: 5 + m / 60,
        observedAt: iso(m),
        resetAt: iso(480),
      })),
      ...series(0, 360, 15, (m) => 50 + 2 * (m / 60), iso(480)).map((o) => ({
        ...o,
        providerId: "p2",
        windowLabel: "w2",
      })),
    ];
    const predictions = predictWindows({ observations, now: NOW });
    expect(predictions.map((p) => `${p.providerId}/${p.windowLabel}`)).toEqual([
      "p1/other",
      "p1/w",
      "p2/w2",
    ]);
    expect(predictions[1].burnRatePerHour).toBeCloseTo(4, 6);
    expect(predictions[2].burnRatePerHour).toBeCloseTo(2, 6);
    expect(predictions.every((p) => p.confidence !== "insufficient")).toBe(true);
  });
});

// MIC-298: a refresh could blank the whole app with
// "RangeError: Invalid time value" from predictWindow. An idle provider
// reports a constant percentage; the least-squares slope of such a series is
// floating-point noise. When the noise lands on a tiny POSITIVE value, the
// exhaustion projection multiplies it out to an epoch far beyond the largest
// representable Date, and `new Date(ms).toISOString()` threw — inside the
// App render, unmounting the entire tree (black window).
describe("predictWindow — runaway exhaustion projection (MIC-298 regression)", () => {
  // The exact series that crashed v0.3.0 in the field: Z.ai "5-hour" at a
  // constant 63%, whose resetAt jumped forward mid-session, splitting the
  // history so the newest segment holds these three samples over ~1.4 min.
  const crashedZai: QuotaObservation[] = [
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:56:29.984Z", resetAt: "2026-09-28T14:54:58.990Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:56:50.675Z", resetAt: "2026-09-28T14:54:58.990Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:57:36.392Z", resetAt: "2026-09-28T14:54:58.990Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:57:36.982Z", resetAt: "2026-09-28T14:54:58.990Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:59:28.203Z", resetAt: "2026-09-28T14:59:12.278Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T11:59:47.138Z", resetAt: "2026-09-28T14:59:12.278Z" },
    { providerId: "zai", windowLabel: "5-hour", usedPercent: 63, observedAt: "2026-09-28T12:00:53.589Z", resetAt: "2026-09-28T14:59:12.278Z" },
  ];
  const crashNow = "2026-09-28T12:01:30.000Z";

  it("does not throw when float noise makes an idle series' burn rate tiny but positive", () => {
    const p = predictWindow({
      observations: crashedZai,
      providerId: "zai",
      windowLabel: "5-hour",
      now: crashNow,
    });
    // The engine segments on the resetAt jump and fits the newest segment,
    // where the constant percentage leaves only rounding noise.
    expect(p.burnRatePerHour).toBeGreaterThan(0);
    expect(p.burnRatePerHour!).toBeLessThan(1e-6);
    // Before the fix this line itself threw; the estimate is now omitted.
    expect(p.estimatedExhaustionAt).toBeUndefined();
    // The bounded projection fields stay intact.
    expect(p.projectedPercentAtReset).toBeCloseTo(63, 6);
    expect(p.risk).toBe("low");
  });

  it("keeps every predictWindows entry representable when replaying the crashing snapshot", () => {
    const observations = [
      ...crashedZai,
      {
        providerId: "openai-codex",
        windowLabel: "5-hour",
        usedPercent: 42.5,
        observedAt: "2026-09-28T11:59:47.138Z",
        resetAt: "2026-09-28T13:59:47.135Z",
      },
    ];
    let predictions: ReturnType<typeof predictWindows>;
    expect(() => {
      predictions = predictWindows({ observations, now: crashNow });
    }).not.toThrow();
    for (const p of predictions!) {
      if (p.estimatedExhaustionAt !== undefined) {
        expect(Number.isFinite(Date.parse(p.estimatedExhaustionAt))).toBe(true);
      }
    }
  });

  it("deterministically omits the exhaustion estimate for a subnormal-but-real burn rate", () => {
    // A strictly increasing series engineered to a slope of ~3e-9 %/h, far
    // below anything measurable: (100 - 63) / 3e-9 ≈ 1.2e10 hours to
    // exhaustion ≈ 4.4e16 ms — beyond MAX_DATE_MS (8.64e15). Not dependent
    // on floating-point rounding of identical percentages.
    const creeping: QuotaObservation[] = [
      { providerId: "p", windowLabel: "w", usedPercent: 63, observedAt: iso(0), resetAt: iso(480) },
      { providerId: "p", windowLabel: "w", usedPercent: 63 + 1e-10, observedAt: iso(2), resetAt: iso(480) },
      { providerId: "p", windowLabel: "w", usedPercent: 63 + 2e-10, observedAt: iso(4), resetAt: iso(480) },
    ];
    const p = predict(creeping)!;
    expect(p.burnRatePerHour).toBeGreaterThan(0);
    expect(p.estimatedExhaustionAt).toBeUndefined();
    expect(p.projectedPercentAtReset).toBeDefined();
  });

  it("still reports exhaustion for an ordinary burn rate", () => {
    const p = predict(series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(480)))!;
    expect(p.estimatedExhaustionAt).toBe("2026-09-28T04:30:00.000Z");
  });
});

// ---------------------------------------------------------------------------
// v0.8 Lane A — threshold estimates (ThresholdEstimate contract).
// ---------------------------------------------------------------------------
describe("predictWindow — threshold estimates", () => {
  it("projects both level crossings when the fit reaches them before the reset", () => {
    // Two samples 15 minutes apart: slope 24%/h, latest 40% at minute 360,
    // reset three hours later. 80 lands at minute 460, 95 at minute 497.5.
    const p = predict([obsAt(345, 34, iso(540)), obsAt(360, 40, iso(540))])!;
    expect(p.thresholds).toBeDefined();
    expect(p.thresholds!.map((t) => t.thresholdPercent)).toEqual([
      NEAR_LIMIT_PERCENT,
      CRITICAL_PERCENT,
    ]);
    const [t80, t95] = p.thresholds!;
    expect(t80.crossesBeforeReset).toBe(true);
    expect(t80.alreadyCrossed).toBeUndefined();
    expectCrossingAt(t80.estimatedAt, 460);
    expect(t95.crossesBeforeReset).toBe(true);
    expectCrossingAt(t95.estimatedAt, 497.5);
  });

  it("reports a zero burn as never reaching either level, with no projected time", () => {
    const p = predict(series(0, 360, 15, () => 42, iso(480)))!;
    expect(p.thresholds).toEqual([
      { thresholdPercent: NEAR_LIMIT_PERCENT, crossesBeforeReset: false },
      { thresholdPercent: CRITICAL_PERCENT, crossesBeforeReset: false },
    ]);
  });

  it("treats a negative-slope clamp (idle) exactly like a zero burn", () => {
    const p = predict(series(0, 360, 15, (m) => 50 - 2 * (m / 60), iso(480)))!;
    expect(p.burnRatePerHour).toBe(0);
    expect(p.thresholds).toEqual([
      { thresholdPercent: NEAR_LIMIT_PERCENT, crossesBeforeReset: false },
      { thresholdPercent: CRITICAL_PERCENT, crossesBeforeReset: false },
    ]);
  });

  it("marks one level already crossed and still projects the other", () => {
    // 12%/h over two hours: latest 85% at minute 360. 80 is an observed
    // fact; 95 lands at minute 410, before the reset at 480.
    const p = predict(
      series(240, 360, 15, (m) => 61 + 12 * ((m - 240) / 60), iso(480)),
    )!;
    const [t80, t95] = p.thresholds!;
    expect(t80).toEqual({
      thresholdPercent: NEAR_LIMIT_PERCENT,
      crossesBeforeReset: true,
      alreadyCrossed: true,
      estimatedAt: iso(360),
    });
    expect(t95.alreadyCrossed).toBeUndefined();
    expect(t95.crossesBeforeReset).toBe(true);
    expectCrossingAt(t95.estimatedAt, 410);
  });

  it("treats the newest sample sitting exactly on a level as already crossed", () => {
    const at80 = predict(
      series(240, 360, 15, (m) => 56 + 12 * ((m - 240) / 60), iso(480)),
    )!;
    expect(at80.thresholds![0].alreadyCrossed).toBe(true);
    expect(at80.thresholds![0].estimatedAt).toBe(iso(360));
    // 95 is still a projection from the same fit: 75 minutes out.
    expectCrossingAt(at80.thresholds![1].estimatedAt, 435);

    const at95 = predict(
      series(240, 360, 15, (m) => 71 + 12 * ((m - 240) / 60), iso(480)),
    )!;
    expect(at95.thresholds!.map((t) => t.alreadyCrossed)).toEqual([true, true]);
    expect(at95.thresholds!.every((t) => t.estimatedAt === iso(360))).toBe(true);
  });

  it("takes the already-crossed shape for both levels on an exhausted window", () => {
    const p = predict(series(0, 360, 15, () => 100, iso(480)))!;
    expect(p.thresholds).toEqual([
      {
        thresholdPercent: NEAR_LIMIT_PERCENT,
        crossesBeforeReset: true,
        alreadyCrossed: true,
        estimatedAt: iso(360),
      },
      {
        thresholdPercent: CRITICAL_PERCENT,
        crossesBeforeReset: true,
        alreadyCrossed: true,
        estimatedAt: iso(360),
      },
    ]);
  });

  it("reports not-before-reset, without a projected time, when the reset lands first", () => {
    // 4%/h from 34%: the 80% crossing sits 11.5h out, far past the reset
    // two hours after the newest sample. The honest statement is that the
    // level is not expected before reset — never a post-reset time.
    const p = predict(series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(480)))!;
    expect(p.thresholds).toEqual([
      { thresholdPercent: NEAR_LIMIT_PERCENT, crossesBeforeReset: false },
      { thresholdPercent: CRITICAL_PERCENT, crossesBeforeReset: false },
    ]);
    expect(p.thresholds!.every((t) => t.estimatedAt === undefined)).toBe(true);
  });

  it("suppresses a crossing that the fit placed in the past without a confirming sample", () => {
    // 18%/h into 79%: the 80% crossing lands 3⅓ minutes after the last
    // sample. Evaluated 25 minutes later no confirming observation has
    // arrived, so the 80% claim is withdrawn while the 95% projection
    // (53⅓ minutes out) stays.
    const late = [
      obsAt(330, 70, iso(480)),
      obsAt(345, 75, iso(480)),
      obsAt(360, 79, iso(480)),
    ];
    const before = predict(late, iso(360))!;
    expect(before.thresholds![0].crossesBeforeReset).toBe(true);
    expectCrossingAt(before.thresholds![0].estimatedAt, 363 + 1 / 3);
    const after = predict(late, iso(385))!;
    expect(after.thresholds![0]).toEqual({
      thresholdPercent: NEAR_LIMIT_PERCENT,
      crossesBeforeReset: undefined,
    });
    expect(after.thresholds![0].estimatedAt).toBeUndefined();
    expect(after.thresholds![1].crossesBeforeReset).toBe(true);
    expectCrossingAt(after.thresholds![1].estimatedAt, 413 + 1 / 3);
  });

  it("omits the levels entirely when no reset bound exists", () => {
    const p = predict(series(0, 360, 15, (m) => 10 + 4 * (m / 60)))!;
    expect(p.burnRatePerHour).toBeCloseTo(4, 6);
    expect(p.thresholds).toBeUndefined();
  });

  it("omits the levels entirely when the reset bound has expired", () => {
    const drifted = series(0, 360, 30, (m) => 10 + 4 * (m / 60)).map((o, i) => ({
      ...o,
      resetAt: i <= 6 ? iso(480) : iso(300), // regressed, and already in the past
    }));
    const p = predict(drifted)!;
    expect(p.basis.resetExpired).toBe(true);
    expect(p.thresholds).toBeUndefined();
  });

  it("omits unfit levels but still reports an observed crossing without a fit", () => {
    // One sample below 80: no burn rate, no reset-anchored claim — nothing
    // knowable, so no levels at all.
    const unfit = predict([obsAt(360, 10, iso(480))])!;
    expect(unfit.burnRatePerHour).toBeUndefined();
    expect(unfit.thresholds).toBeUndefined();

    // One sample at 85%: the 80% crossing is an observed fact that needs no
    // fit; the 95% level stays omitted (nothing knowable).
    const observed = predict([obsAt(360, 85, iso(480))])!;
    expect(observed.burnRatePerHour).toBeUndefined();
    expect(observed.confidence).toBe("insufficient");
    expect(observed.thresholds).toEqual([
      {
        thresholdPercent: NEAR_LIMIT_PERCENT,
        crossesBeforeReset: true,
        alreadyCrossed: true,
        estimatedAt: iso(360),
      },
    ]);
  });

  it("never fabricates a date when the crossing horizon escapes the Date range", () => {
    // Slope ~3e-9 %/h: the 80% crossing sits ~5.7e9 hours out — beyond
    // MAX_DATE_MS and far past the reset. The level must read "not before
    // reset", and no toISOString may throw.
    const creeping: QuotaObservation[] = [
      { providerId: "p", windowLabel: "w", usedPercent: 63, observedAt: iso(0), resetAt: iso(480) },
      { providerId: "p", windowLabel: "w", usedPercent: 63 + 1e-10, observedAt: iso(2), resetAt: iso(480) },
      { providerId: "p", windowLabel: "w", usedPercent: 63 + 2e-10, observedAt: iso(4), resetAt: iso(480) },
    ];
    let p: ReturnType<typeof predictWindow>;
    expect(() => {
      p = predict(creeping)!;
    }).not.toThrow();
    expect(p!.thresholds!.every((t) => t.crossesBeforeReset === false)).toBe(true);
    expect(p!.thresholds!.every((t) => t.estimatedAt === undefined)).toBe(true);
  });

  it("is deterministic: identical inputs produce identical threshold output", () => {
    const observations = series(0, 360, 15, (m) => 10 + 4 * (m / 60), iso(480));
    const first = predict(observations)!;
    const second = predict(observations)!;
    expect(JSON.stringify(first.thresholds)).toBe(
      JSON.stringify(second.thresholds),
    );
    // Anchored on the newest observation: advancing `now` inside the fresh
    // window changes nothing about the stored estimates.
    const later = predict(observations, iso(385))!;
    expect(later.thresholds).toEqual(first.thresholds);
  });
});

describe("threshold constants (v0.8 single-sourcing)", () => {
  it("pins the canonical levels the engine and presentation share", () => {
    expect(NEAR_LIMIT_PERCENT).toBe(80);
    expect(CRITICAL_PERCENT).toBe(95);
  });
});
