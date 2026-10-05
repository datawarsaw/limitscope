import { useEffect, useState } from "react";

/**
 * Wall-clock ticks for purely local, time-derived UI (reset countdowns).
 * 30s keeps minute-granularity readouts honest without visible flicker and
 * without touching any provider data.
 */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}
