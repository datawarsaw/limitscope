//! Reset plausibility bounds (v0.8 Lane B runtime trust).
//!
//! A provider-announced `reset_at` is trusted only when it is *plausible*
//! for the window label it rides on. The check is pure — a label table plus
//! arithmetic against the caller's clock; no IO, no state, no provider
//! specifics. It sits at the single normalization chokepoint
//! (`runtime::limits_from`), evaluated per window from that window's own
//! label and stamp, so a rejected bound on one window never affects its
//! siblings. On rejection the bound is dropped, not trusted: the window
//! keeps its data with `reset_at: None` — exactly the Z.ai epoch-bounds
//! degradation (`zai::epoch_ms_to_rfc3339`), generalized to every provider.
//!
//! Downstream, a missing bound is honest degradation, all existing behavior:
//! history records the observation without a bound; prediction loses its
//! reset anchor (confidence drops, projections stay hidden); notifications
//! treat it as "no new cycle". Hydration (`last_good::hydrate`) re-applies
//! this same check to stored bounds at cold start, so a bound persisted by
//! an older version cannot re-enter the runtime.
//!
//! A legitimate announced reset is never "suspicious" and never gated here:
//! this validates the timestamp a provider announces; the separate
//! suspicious-drop confirmation flow in `runtime.rs` validates an observed
//! usage drop and explicitly exempts forward reset moves.

use chrono::DateTime;

/// Shared clock-skew tolerance: a reset up to five minutes into the past is
/// provider lag, not corruption — the window is simply already expired. Same
/// value as `history::QUOTA_HISTORY_CLOCK_SKEW_TOLERANCE_MS` and the engine's
/// `clockSkewToleranceMs` default.
const CLOCK_SKEW_TOLERANCE_MS: i64 = 5 * 60_000;

const HOUR_MS: i64 = 60 * 60_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// True when `reset_at` is a plausible reset instant for `window_label`:
/// it parses as RFC-3339, sits after `now − CLOCK_SKEW_TOLERANCE_MS`, and
/// stays inside the label's maximum horizon. The horizon table is
/// deterministic and locale-fixed (trimmed, lowercased):
///
/// | label class                            | max horizon |
/// |----------------------------------------|-------------|
/// | `N hours` / `Nh` with `1 ≤ N ≤ 24`     | 2·N h + 1 h |
/// | `hourly`, `1 hour`                     | 3 h         |
/// | `daily`, `1 day`, `24 hours`           | 48 h        |
/// | `weekly`                               | 15 d        |
/// | `monthly`                              | 35 d        |
/// | anything else (conservative fallback)  | 35 d        |
pub(crate) fn plausible_reset_at(window_label: &str, reset_at: &str, now_ms: i64) -> bool {
    let Some(reset_ms) = DateTime::parse_from_rfc3339(reset_at)
        .ok()
        .map(|parsed| parsed.timestamp_millis())
    else {
        return false;
    };
    if reset_ms <= now_ms - CLOCK_SKEW_TOLERANCE_MS {
        return false;
    }
    if reset_ms >= now_ms.saturating_add(max_horizon_ms(window_label)) {
        return false;
    }
    true
}

/// The maximum plausible reset horizon for one window label.
fn max_horizon_ms(window_label: &str) -> i64 {
    let label = window_label.trim().to_lowercase();
    // Explicit rows first: "24 hours" is the daily row, not the generic
    // N-hours parse (which would allow 49 h).
    match label.as_str() {
        "hourly" | "1 hour" => return 3 * HOUR_MS,
        "daily" | "1 day" | "24 hours" => return 48 * HOUR_MS,
        "weekly" => return 15 * DAY_MS,
        "monthly" => return 35 * DAY_MS,
        _ => {}
    }
    if let Some(hours) = hour_count(&label) {
        return (2 * hours + 1) * HOUR_MS;
    }
    35 * DAY_MS
}

/// Parses the `N` of `N hours` / `Nh` labels when `1 ≤ N ≤ 24`. The
/// production vocabulary spells the hour class both ways — Codex maps its
/// five-hour primary window to `5-hour` (and `1-hour`, `codex.rs
/// window_label`), while other providers announce `5 hours` — so the
/// separator/suffix variants share one parse instead of silently falling
/// through to the 35-day generic horizon.
fn hour_count(label: &str) -> Option<i64> {
    let digits = label
        .strip_suffix("hours")
        .or_else(|| label.strip_suffix("hour"))
        .or_else(|| label.strip_suffix('h'))
        .map(|rest| rest.trim_end_matches([' ', '-']))?;
    let hours = digits.parse::<i64>().ok()?;
    (1..=24).contains(&hours).then_some(hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{SecondsFormat, TimeZone, Utc};

    fn now_ms() -> i64 {
        Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0)
            .unwrap()
            .timestamp_millis()
    }

    /// A reset stamp `hours` hours from the fixed now.
    fn reset_hours_from_now(hours: f64) -> String {
        let ms = now_ms() + (hours * HOUR_MS as f64).round() as i64;
        DateTime::from_timestamp_millis(ms)
            .unwrap()
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    // Each table row pins its horizon: exactly at the horizon is outside
    // (`reset_at < now + horizon` is strict), one millisecond inside is kept.
    #[test]
    fn table_rows_pin_their_horizons() {
        let rows: [(&str, i64); 11] = [
            ("5 hours", 11),
            ("5h", 11),
            // The Codex vocabulary spells the same class hyphenated; the
            // five-hour primary window must get the 2·N+1 h horizon, not the
            // 35-day fallback.
            ("5-hour", 11),
            ("1-hour", 3),
            ("1h", 3),
            ("hourly", 3),
            ("1 hour", 3),
            ("daily", 48),
            ("1 day", 48),
            ("weekly", 15 * 24),
            ("monthly", 35 * 24),
        ];
        for (label, horizon_h) in rows {
            assert!(
                !plausible_reset_at(label, &reset_hours_from_now(horizon_h as f64), now_ms()),
                "{label}: a reset exactly at the {horizon_h}h horizon must be rejected"
            );
            assert!(
                plausible_reset_at(label, &reset_hours_from_now(horizon_h as f64 - 0.001), now_ms()),
                "{label}: a reset just inside the {horizon_h}h horizon must be plausible"
            );
        }
    }

    #[test]
    fn the_explicit_daily_row_wins_over_the_generic_hours_parse() {
        // "24 hours" is the daily row (48 h); the generic N-hours parse
        // would have allowed 49 h.
        assert!(!plausible_reset_at(
            "24 hours",
            &reset_hours_from_now(48.5),
            now_ms()
        ));
        assert!(plausible_reset_at(
            "24 hours",
            &reset_hours_from_now(47.0),
            now_ms()
        ));
    }

    #[test]
    fn unknown_labels_fall_back_to_the_conservative_35_day_horizon() {
        for label in ["3 days", "48 hours", "quarterly", "", "5-hour limit", "h"] {
            assert!(
                plausible_reset_at(label, &reset_hours_from_now(24.0 * 30.0), now_ms()),
                "{label:?}: 30 days out must stay plausible on the fallback"
            );
            assert!(
                !plausible_reset_at(label, &reset_hours_from_now(24.0 * 36.0), now_ms()),
                "{label:?}: 36 days out must be rejected on the fallback"
            );
        }
    }

    #[test]
    fn the_hours_parse_rejects_out_of_range_counts() {
        // N must satisfy 1 ≤ N ≤ 24; anything else falls back (35 d), it
        // never tightens to a bogus 2·N+1 horizon.
        assert!(plausible_reset_at(
            "0 hours",
            &reset_hours_from_now(24.0),
            now_ms()
        ));
        assert!(plausible_reset_at(
            "48 hours",
            &reset_hours_from_now(24.0 * 20.0),
            now_ms()
        ));
    }

    #[test]
    fn past_stamps_only_survive_inside_the_skew_tolerance() {
        assert!(
            plausible_reset_at("weekly", &reset_hours_from_now(-0.01), now_ms()),
            "a reset seconds into the past is provider lag, not corruption"
        );
        // Exactly now − 5 min is outside (`now − tolerance < reset_at` is
        // strict); anything older is rejected.
        assert!(!plausible_reset_at(
            "weekly",
            &reset_hours_from_now(-5.0 / 60.0),
            now_ms()
        ));
        assert!(!plausible_reset_at(
            "weekly",
            &reset_hours_from_now(-1.0),
            now_ms()
        ));
    }

    #[test]
    fn unparseable_stamps_are_never_plausible() {
        assert!(!plausible_reset_at("weekly", "soon", now_ms()));
        assert!(!plausible_reset_at("weekly", "", now_ms()));
        assert!(!plausible_reset_at("weekly", "2026-13-40T99:00:00Z", now_ms()));
    }

    #[test]
    fn label_matching_is_locale_fixed_and_trimmed() {
        assert!(plausible_reset_at(
            "  WEEKLY  ",
            &reset_hours_from_now(24.0 * 10.0),
            now_ms()
        ));
        assert!(plausible_reset_at(
            "Weekly",
            &reset_hours_from_now(24.0 * 10.0),
            now_ms()
        ));
        assert!(!plausible_reset_at(
            "WEEKLY",
            &reset_hours_from_now(24.0 * 20.0),
            now_ms()
        ));
    }

    #[test]
    fn a_plausible_reset_is_accepted_end_to_end() {
        assert!(plausible_reset_at(
            "5 hours",
            &reset_hours_from_now(5.0),
            now_ms()
        ));
        assert!(plausible_reset_at(
            "Weekly",
            &reset_hours_from_now(24.0 * 7.0),
            now_ms()
        ));
    }
}
