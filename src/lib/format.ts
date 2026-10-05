const timeFormat = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
});
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dateFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
});

const DAY_MS = 24 * 60 * 60 * 1000;

function startOfDay(date: Date): number {
  const copy = new Date(date);
  copy.setHours(0, 0, 0, 0);
  return copy.getTime();
}

export function formatTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return timeFormat.format(date);
}

export function formatResetTime(iso: string, now: Date = new Date()): string {
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return "—";
  const daysAhead = Math.round(
    (startOfDay(target) - startOfDay(now)) / DAY_MS,
  );
  if (daysAhead <= 0) return timeFormat.format(target);
  if (daysAhead <= 7) {
    return `${weekdayFormat.format(target)} ${timeFormat.format(target)}`;
  }
  return dateFormat.format(target);
}

/**
 * Compact relative age of a timestamp ("just now", "12m ago", "3h ago",
 * "9d ago"). A future-dated stamp (clock skew) reads as "just now".
 */
export function formatAge(iso: string, now: Date = new Date()): string {
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return "unknown";
  const minutes = Math.floor(Math.max(0, now.getTime() - target.getTime()) / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/**
 * Compact countdown until a future timestamp ("5d 13h", "2h 17m", "18m",
 * "<1m"), truncated so it never overstates the remaining time. Returns null
 * once the timestamp has passed — the caller decides how to present an
 * expired reset.
 */
export function formatCountdown(iso: string, now: Date = new Date()): string | null {
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return null;
  const remainingMs = target.getTime() - now.getTime();
  if (remainingMs <= 0) return null;
  const totalMinutes = Math.floor(remainingMs / 60000);
  if (totalMinutes < 1) return "<1m";
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

/**
 * Full reset readout for a limit window: absolute time plus a locally
 * derived countdown ("Resets Sat 9:15 am · in 5d 13h"). A reset that has
 * already passed reads "Reset time passed" instead of a negative duration.
 * An invalid timestamp returns an empty string so callers can omit the row.
 */
export function formatResetLine(iso: string, now: Date = new Date()): string {
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return "";
  if (target.getTime() <= now.getTime()) {
    return "Reset time passed";
  }
  const countdown = formatCountdown(iso, now);
  if (countdown === null) {
    return `Resets ${formatResetTime(iso, now)}`;
  }
  return `Resets ${formatResetTime(iso, now)} · in ${countdown}`;
}
