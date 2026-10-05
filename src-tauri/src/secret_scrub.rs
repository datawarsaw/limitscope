//! Exact-value secret scrubbing at the provider error boundary.
//!
//! Defense-in-depth for the one free-form string that reaches the WebView on
//! a failed refresh (`ProviderUsageDto.error`): while a credential value is
//! in scope inside a provider fetch, any exact occurrence of that value in
//! the fetch's error message is replaced before the failure leaves the
//! adapter. The diagnostics bundle additionally discards provider error
//! messages outright and regex-redacts every other free-form field, and
//! usage exports regex-redact credential shapes; this layer covers what a
//! shape-based pass cannot: a credential value that appears verbatim (for
//! example an upstream echoing an authenticated request's own token back
//! inside an error detail) without matching any credential *shape*.
//!
//! Contract:
//! - exact-value only: literal byte matching, never regex guessing;
//! - borrowed seeds only: the scrubber never copies, stores, hashes, or logs
//!   a secret, and the adapter call sites pass references that are already
//!   alive for the whole fetch — credential lifetimes do not change;
//! - deterministic: seeds are deduplicated and applied longest-first;
//! - bounded: seeds shorter than [`MIN_SEED_LEN`] are ignored (they cannot
//!   be meaningful credentials and would mangle benign text), and the
//!   replacement is a constant placeholder, so the removed value can never
//!   be reconstructed from the output.

use crate::provider_error::ProviderError;

/// The fixed replacement token. Shared with the diagnostics sanitizer's
/// vocabulary so a scrubbed value is not distinguishable from a
/// shape-redacted one.
pub(crate) const PLACEHOLDER: &str = "[REDACTED]";

/// Seeds below this length are ignored. Every credential class the provider
/// backends handle (API keys, OAuth tokens, session JWTs) is far longer, and
/// replacing a short literal would corrupt ordinary prose more than it
/// protects.
const MIN_SEED_LEN: usize = 8;

/// Replaces every exact occurrence of every known secret value in `text`
/// with [`PLACEHOLDER`]. Benign neighboring text — including text that merely
/// resembles a seed — is left byte-identical. No-op when no usable seed is
/// given (empty, too short, or all duplicates).
pub(crate) fn scrub_text(text: &str, seeds: &[&str]) -> String {
    let mut usable: Vec<&str> = seeds
        .iter()
        .copied()
        .filter(|seed| seed.len() >= MIN_SEED_LEN)
        .collect();
    if usable.is_empty() {
        return text.to_string();
    }
    // Longest-first so a seed that is a prefix of another never leaves a
    // partial value behind; the secondary key makes the order of the caller's
    // slice irrelevant. `dedup` then collapses exact duplicates.
    usable.sort_unstable_by_key(|seed| (std::cmp::Reverse(seed.len()), *seed));
    usable.dedup();
    let mut scrubbed = text.to_string();
    for seed in usable {
        if scrubbed.contains(seed) {
            scrubbed = scrubbed.replace(seed, PLACEHOLDER);
        }
    }
    scrubbed
}

/// The same scrub applied to the shared provider failure before it is
/// published: only `message` is touched; code, status, retry verdict, and
/// the internal identity hint pass through untouched.
pub(crate) fn scrub_provider_error(mut error: ProviderError, seeds: &[&str]) -> ProviderError {
    error.message = scrub_text(&error.message, seeds);
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "S3cr3t-Value-0123456789abcdef";
    const OTHER_SECRET: &str = "0auth-t0ken-9876543210fedcba";

    fn assert_scrubbed(input: String) -> String {
        let output = scrub_text(&input, &[SECRET]);
        assert!(!output.contains(SECRET), "leaked: {output}");
        assert!(output.contains(PLACEHOLDER), "not scrubbed: {output}");
        output
    }

    // ---------- the adversarial matrix ----------

    #[test]
    fn alone_is_fully_replaced() {
        assert_eq!(assert_scrubbed(SECRET.to_string()), PLACEHOLDER);
    }

    #[test]
    fn embedded_in_url_query_text_is_replaced() {
        let output = assert_scrubbed(format!(
            "GET https://api.example.com/v1/usage?token={SECRET}&page=2 failed"
        ));
        assert_eq!(
            output,
            "GET https://api.example.com/v1/usage?token=[REDACTED]&page=2 failed"
        );
    }

    #[test]
    fn authorization_like_text_is_replaced() {
        let output = assert_scrubbed(format!("Authorization: Bearer {SECRET}"));
        assert_eq!(output, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn inside_a_json_string_is_replaced() {
        let output = assert_scrubbed(format!("{{\"error\":\"key {SECRET} refused\"}}"));
        assert_eq!(output, "{\"error\":\"key [REDACTED] refused\"}");
    }

    #[test]
    fn repeated_occurrences_are_all_replaced() {
        let input = format!("{SECRET} then {SECRET} then {SECRET}");
        let output = assert_scrubbed(input);
        assert_eq!(
            output.matches(PLACEHOLDER).count(),
            3,
            "every occurrence must go: {output}"
        );
    }

    #[test]
    fn prefix_and_suffix_context_survive() {
        let output = assert_scrubbed(format!("prefix-{SECRET}-suffix"));
        assert_eq!(output, "prefix-[REDACTED]-suffix");
    }

    #[test]
    fn unicode_surroundings_survive() {
        let output = assert_scrubbed(format!("错误：令牌 {SECRET} 无效 🚫"));
        assert_eq!(output, "错误：令牌 [REDACTED] 无效 🚫");
    }

    #[test]
    fn multiple_known_secrets_are_all_replaced() {
        let input = format!("{SECRET} mixed with {OTHER_SECRET}");
        let output = scrub_text(&input, &[SECRET, OTHER_SECRET]);
        assert!(
            !output.contains(SECRET) && !output.contains(OTHER_SECRET),
            "{output}"
        );
        // Seed order must not matter: the result is deterministic.
        let reordered = scrub_text(&input, &[OTHER_SECRET, SECRET]);
        assert_eq!(output, reordered);
    }

    #[test]
    fn empty_seed_is_ignored_and_injects_nothing() {
        // An empty literal would otherwise "match" at every char boundary.
        assert_eq!(scrub_text("ordinary failure", &[""]), "ordinary failure");
        assert_eq!(
            scrub_text("ordinary failure", &["", SECRET]),
            "ordinary failure"
        );
    }

    #[test]
    fn duplicate_seeds_behave_like_one() {
        let input = format!("once {SECRET} twice {SECRET}");
        assert_eq!(
            scrub_text(&input, &[SECRET, SECRET]),
            scrub_text(&input, &[SECRET])
        );
    }

    #[test]
    fn seed_below_the_minimum_length_is_ignored() {
        // Too short to be a credential: replacing it would corrupt prose.
        assert_eq!(scrub_text("abc and abc", &["abc"]), "abc and abc");
    }

    #[test]
    fn benign_text_is_byte_identical_when_no_seed_matches() {
        assert_eq!(
            scrub_text("ordinary failure", &[SECRET]),
            "ordinary failure"
        );
        // Near-miss text that merely resembles the secret survives too.
        let near_miss = "S3cr3t-Value-0123456789abcdeX";
        assert_eq!(scrub_text(near_miss, &[SECRET]), near_miss);
    }

    #[test]
    fn seed_longer_than_the_text_changes_nothing() {
        assert_eq!(
            scrub_text("short", &["a-very-long-seed-value-0123456789"]),
            "short"
        );
    }

    #[test]
    fn regex_metacharacters_match_literally_only() {
        let seed = "a.b*c?d[e]f";
        assert_eq!(scrub_text("a.b*c?d[e]f", &[seed]), PLACEHOLDER);
        assert_eq!(scrub_text("aXbXcXdXeXf", &[seed]), "aXbXcXdXeXf");
    }

    #[test]
    fn multibyte_seed_is_replaced_exactly() {
        let seed = "🔒🔑🗝️-secret-1234";
        let output = scrub_text(&format!("token {seed} expired"), &[seed]);
        assert_eq!(output, "token [REDACTED] expired");
    }

    #[test]
    fn overlapping_seeds_apply_longest_first() {
        let long = "abcdefghijklmnop";
        let short = "abcdefgh";
        let output = scrub_text(&format!("value {long} end"), &[short, long]);
        assert_eq!(output, "value [REDACTED] end");
        // The shorter seed alone is still removed when it appears alone.
        let output = scrub_text(&format!("value {short} end"), &[short, long]);
        assert_eq!(output, "value [REDACTED] end");
    }

    #[test]
    fn preexisting_placeholder_text_is_preserved() {
        // A seed may itself contain the placeholder; a benign literal
        // placeholder in the text must never grow into a false match.
        let seeded = "[REDACTED]-payload-marker";
        let text = "earlier [REDACTED] marker";
        assert_eq!(scrub_text(text, &[seeded]), text);
    }

    #[test]
    fn replacement_cannot_reconstruct_the_secret() {
        // The placeholder is a constant; splicing output fragments back
        // together can only ever produce the placeholder again.
        let output = assert_scrubbed(format!("a{SECRET}b{SECRET}c"));
        assert_eq!(output, format!("a{PLACEHOLDER}b{PLACEHOLDER}c"));
        assert!(!output.contains(&SECRET[..SECRET.len() / 2]), "{output}");
    }

    #[test]
    fn scrubbing_is_deterministic_and_total_over_hostile_input() {
        // Control characters, mixed boundary shapes, and repeated runs must
        // neither panic nor vary between runs.
        let hostile = format!("汤\u{0}\u{7}{SECRET}\u{1f}\t{SECRET}{SECRET}");
        let first = scrub_text(&hostile, &[SECRET]);
        let second = scrub_text(&hostile, &[SECRET]);
        assert_eq!(first, second);
        assert!(!first.contains(SECRET));
        assert_eq!(first.matches(PLACEHOLDER).count(), 3);
    }

    #[test]
    fn bounded_scan_over_large_text_completes() {
        // Far above any real message budget (per-message caps sit around
        // 200–512 chars): a 64 KiB text against 16 seeds is still a bounded,
        // fast literal scan.
        let seeds: Vec<String> = (0..16).map(|i| format!("seed-{i:02}-0123456789")).collect();
        let text = "filler ".repeat(1024 * 12) + &seeds[7];
        let refs: Vec<&str> = seeds.iter().map(String::as_str).collect();
        let output = scrub_text(&text, &refs);
        assert!(!output.contains(seeds[7].as_str()));
        assert!(output.ends_with(PLACEHOLDER));
    }

    // ---------- the ProviderError wrapper ----------

    #[test]
    fn provider_error_scrub_touches_only_the_message() {
        let error = ProviderError::http_failure(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            format!("refused token {SECRET} with prejudice"),
        )
        .with_identity_hint(Some("key:1234".to_string()));
        let scrubbed = scrub_provider_error(error, &[SECRET]);
        assert_eq!(scrubbed.code, "unexpected_response");
        assert_eq!(scrubbed.http_status, Some(503));
        assert_eq!(scrubbed.transient, Some(true));
        assert_eq!(scrubbed.identity_hint.as_deref(), Some("key:1234"));
        assert!(!scrubbed.message.contains(SECRET));
        assert!(scrubbed
            .message
            .contains("refused token [REDACTED] with prejudice"));
    }

    #[test]
    fn provider_error_scrub_without_usable_seeds_is_a_no_op() {
        let error = ProviderError::new("auth_invalid", "rejected as sent");
        let scrubbed = scrub_provider_error(error.clone(), &[]);
        assert_eq!(scrubbed.message, error.message);
    }
}
