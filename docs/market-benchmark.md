# Rate Limits market benchmark

- **Research date:** 2026-09-27
- **Scope:** current products that monitor AI coding-agent usage, subscription quota, reset windows, or costs.
- **Rate Limits interpretation:** the checked-out product baseline, not uncommitted provider work. Claude and Grok remain mock-only in the current README/release baseline even though discovery files for both exist in the working tree.

## Evidence policy

- **Primary/vendor**: the product's repository, official site, package, or App Store listing. Vendor feature language is labeled as such and is not treated as independent proof.
- **Secondary**: a current aggregator, review, or roundup. Useful for discovery, weaker for capability claims.
- **UNVERIFIED**: the current source set did not establish the field. This is a research result, not a negative claim.
- `Yes` means the source explicitly supports the capability. `No` means the source explicitly excludes it or the product's stated scope excludes it.
- **Multiple windows** means multiple quota/limit windows (for example 5-hour plus weekly), not multiple OS windows.

## Category map

- **A. API cost/token analytics:** estimates or aggregates token consumption and list-price cost from local logs or billing data.
- **B. subscription quota monitoring:** shows plan/entitlement consumption reported by a provider or inferred as a plan limit.
- **C. reset-window monitoring:** shows when a quota window resets and/or how long remains.
- **D. tray/menu-bar utility:** lives in the Windows tray or macOS menu bar for glanceable access.
- **E. historical analytics:** stores or visualizes usage over time.

The core market for Rate Limits is **B + C + D**. A and E are adjacent capabilities, not substitutes for live plan quota and reset visibility.

## Benchmark table

| Product | Category | Platform | Windows support | macOS support | System tray/menu bar | Supported providers | Codex | Claude | OpenCode | Z.ai | Grok | Gemini/Antigravity | Subscription quota | Reset timestamp | Reset countdown | Multiple windows | Historical usage | Cost tracking | Prediction/projection | Stale-data handling | Offline/local-first | Credential model | Open source | License | Activity/maintenance | Current version / recency | Notable UX pattern |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Rate Limits** | B+C+D | Windows native Tauri 2/WebView2 | Yes | No current build | Yes: Open / Refresh / Quit | Codex, Z.ai, OpenCode Go, Antigravity cache; Claude/Grok mock in baseline | Yes, live | No in baseline (mock) | Yes, OpenCode Go live | Yes, live | No in baseline (mock) | Antigravity cached, freshness-aware | Yes | Yes, normalized RFC 3339 `resetAt` | No current UI | Yes, normalized provider windows | No; quota data is in memory | No | No | Last-good retained per card; footer stale after 2x refresh interval; Antigravity 24h freshness verdict | Yes for local credential/cache handling; live fetches need network | Read-only local credentials in Rust; WebView receives normalized windows only | Repository source; distribution license not stated in retrieved docs | UNVERIFIED | Active repository work; current checkout is dirty | v0.2.1 release baseline | Compact dark dashboard, thin progress bars, aligned reset times, tray-first workflow [L1-L4] |
| **TokenTracker (xiufengsun)** | A+B+E | Local dashboard plus native apps claimed | Desktop apps claimed; packaging UNVERIFIED | Desktop apps claimed; packaging UNVERIFIED | No tray claim; local dashboard | 31 coding tools including Claude Code, Codex, Cursor, Gemini, DeepSeek Harness [S1-S2] | Yes | Yes | UNVERIFIED | UNVERIFIED | UNVERIFIED | Gemini yes; Antigravity UNVERIFIED | Yes; provider quota checks use local credentials [S1] | Quota/limits display claimed; exact timestamp semantics UNVERIFIED | UNVERIFIED | UNVERIFIED | Usage/cost tracking claimed; detail UNVERIFIED | Yes, estimated cost [S1-S2] | UNVERIFIED | Staleness semantics UNVERIFIED | Local-first; never reads prompts; telemetry can be disabled [S1] | Reuses credentials already on the machine | Yes, GitHub repository | UNVERIFIED | Current repository/site retrieved; exact commit dates UNVERIFIED | Version UNVERIFIED | Privacy-first local dashboard with quota and cost together [S1] |
| **TokenTracker (pitimon same-name alternate)** | A+B+E | Cross-platform CLI plus local web dashboard | Yes in principle (CLI) | Yes | No; loopback browser dashboard | 20+ tools including Claude Code, Codex, Cursor, Gemini CLI, Antigravity, OpenCode, Z.AI, Grok Build [S12] | Yes | Yes | Yes | Yes | Grok Build yes; Grok CLI UNVERIFIED | Gemini/Antigravity yes | Yes; quota chips for 5h and weekly windows | Yes; Limits page exposes reset data | Yes; Limits page countdowns (vendor claim) | Yes, 24h/day/7d/30d/total/custom lenses | Yes, trend chart, heatmap, project/day tables | Yes, 2,200+ models priced from LiteLLM | UNVERIFIED | Pricing-missing models are badged rather than counted as $0 | Local-first; no account/telemetry; offline price snapshot | Documented outbound calls for prices and provider quota/auth; exact handling UNVERIFIED | Yes | MIT | npm package `@ipv9/tokentracker-cli`; active maintenance UNVERIFIED | Version/commit recency UNVERIFIED | Zero-config dashboard with auto-installed hooks [S12] |
| **TokenBar / Syrtis (Nanako)** | A+B+C+D+E | Native Swift macOS app; macOS 14+, Apple Silicon | No | Yes | Yes, native `NSStatusItem` | 25+ agents; Claude Code, Codex, Cursor, OpenCode, Gemini CLI explicitly named [S3-S4] | Yes | Yes | Yes | UNVERIFIED | UNVERIFIED | Gemini yes; Antigravity UNVERIFIED | Yes; optional OAuth quota cards [S3-S4] | Reset display claimed in listing/secondary evidence; exact UI UNVERIFIED | UNVERIFIED | Session/weekly/credit meters and seven data lenses; quota-window detail UNVERIFIED | Yes; year contribution graph and history preserved across rename | Yes, local session-log cost | Pace projections on OAuth quota cards (vendor claim) | Failed refresh keeps last known value [S3] | Local logs; no telemetry/account; quota lookup needs network | OAuth credentials already on the device | Yes | MIT | Active development; Sparkle updates and beta channel per vendor | v2.0 rename; current build version UNVERIFIED | Liquid Glass popover, 3D contribution graph, RunCat-style motion, keyboard control [S3-S4, S11] |
| **QuotaBar (chilohwei)** | B+C+D | macOS menu bar | No current evidence | Yes | Yes | Claude Code, Codex, Cursor [S5] | Yes | Yes | No evidence | No evidence | No evidence | No evidence | Yes; official APIs refresh quota | Exact reset timestamp semantics UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | No evidence | UNVERIFIED | UNVERIFIED | Hybrid: reads/writes local tool config and calls official APIs | Local config plus provider API credentials; account switching | Yes, GitHub repository | UNVERIFIED | Activity UNVERIFIED | Version UNVERIFIED | Quick switching between local accounts [S5] |
| **QuotaBar (QuotaBar/QuotaBar)** | A+B+C+D | macOS menu-bar meter | UNVERIFIED | Yes | Yes | 22 providers; Claude, Codex, Gemini, Cursor, Grok named [S6-S7] | Yes | Yes | UNVERIFIED | UNVERIFIED | Yes | Gemini yes; Antigravity UNVERIFIED | Yes | Yes; reset times shown [S6] | UNVERIFIED | UNVERIFIED | UNVERIFIED | Yes, locally estimated costs [S6] | UNVERIFIED | UNVERIFIED | Cost estimation local; quota transport UNVERIFIED | UNVERIFIED | Yes, GitHub repository | UNVERIFIED | Activity UNVERIFIED | Version UNVERIFIED | Broad provider menu-bar meter [S6-S7] |
| **ccusage** | A+E | Node CLI; Windows/macOS/Linux in principle | In principle; explicit support UNVERIFIED | In principle; explicit support UNVERIFIED | No | Claude Code primary; Codex variant/support referenced [S8-S10] | Referenced, detail UNVERIFIED | Yes | UNVERIFIED | No evidence | No evidence | UNVERIFIED | No server-plan quota; inferred from local JSONL | No server reset timestamp; inferred 5-hour blocks | No | Daily/monthly reports and inferred blocks | Yes | Yes, list-price estimates | `blocks --live` burn rate; formal projection UNVERIFIED | Offline data; pricing gaps can be surfaced | Yes; zero network and zero auth in documented scope | None for local JSONL parsing | Yes | UNVERIFIED | Active/widely used baseline (~18k stars in secondary source); exact maintenance recency UNVERIFIED | Version UNVERIFIED | Terminal reports and live block burn [S8-S10] |
| **Agenton** | UNVERIFIED | No identifiable product/repo found in current search | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | No verifiable listing found | Version UNVERIFIED | Identity unresolved; do not attribute features |
| **CodexBar** | A+B+C+D+E | iOS App Store companion | No | No | No | AI coding tools; exact provider list UNVERIFIED [S15] | Name suggests yes; UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | Yes; rate limits and budget tracking claimed | Exact reset semantics UNVERIFIED | UNVERIFIED | UNVERIFIED | Usage/history claimed | Yes; spending and budget tracking claimed | Budget tracking only; projection UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | No evidence | Proprietary App Store listing | Current App Store listing | Version UNVERIFIED | Mobile companion for limits, spending, tokens, budgets [S15] |
| **cc-hdrm** | B+C+D | macOS menu-bar app | UNVERIFIED | Yes | Yes | Claude Code [S16] | No evidence | Yes | No evidence | No evidence | No evidence | No evidence | Yes; remaining token headroom claimed | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | v1.3 in Show HN evidence; maintenance UNVERIFIED | Version v1.3 | Headroom-focused Claude meter [S16] |
| **Usage4Claude** | B+C+D | macOS menu-bar utility | UNVERIFIED | Yes | Yes | Claude Code [S16] | No evidence | Yes | No evidence | No evidence | No evidence | No evidence | Yes; 5-hour quota indicators claimed | UNVERIFIED | UNVERIFIED | Multiple display modes claimed | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | Open-source claim in secondary roundup; repository UNVERIFIED | UNVERIFIED | Secondary roundup only | Version UNVERIFIED | Color indicators and display modes [S16] |
| **SessionWatcher** | B | macOS Swift utility | UNVERIFIED | Yes | No evidence | Claude Code [S16] | No evidence | Yes | No evidence | No evidence | No evidence | No evidence | Yes; quota monitor claim | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | Single secondary mention only | Version UNVERIFIED | Lightweight Swift quota watcher [S16] |
| **claude-spend** | A | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | Claude implied by name; UNVERIFIED [S16] | No evidence | Name suggests yes; UNVERIFIED | No evidence | No evidence | No evidence | No evidence | No evidence | No evidence | No evidence | UNVERIFIED | UNVERIFIED | API cost tracking claimed in secondary roundup | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | Single secondary mention only | Version UNVERIFIED | Lead only; not benchmarkable [S16] |

### Identity and confidence notes

1. **TokenTracker is ambiguous.** The current search result points to `xiufengsun/TokenTracker` and its product site [S1-S2]. A separate `pitimon/TokenTracker` project surfaced in the delegated research [S12]. They are kept as separate rows because their provider and product claims differ.
2. **TokenBar and Syrtis are one product line.** The official site says the macOS product was named TokenBar before v2.0 and is now Syrtis [S4, S11]. The benchmark keeps both names so people searching either name can find the evidence.
3. **QuotaBar is ambiguous.** `chilohwei/QuotaBar` and `QuotaBar/QuotaBar` are separate projects [S5-S7]. The former is narrower and account-switching oriented; the latter advertises 22 providers and reset times.
4. **Agenton is unresolved.** No product identity or primary source was established by the current search. It should not be used as a feature benchmark until a URL or repository is supplied.
5. `CodexBar` here means the iOS App Store product surfaced by the current search [S15], not necessarily a macOS menu-bar project of the same name.

## Rate Limits current state

| Dimension | Current checked-out state | Evidence | Competitive read |
|---|---|---|---|
| Windows native tray utility | Tauri 2/WebView2 app with tray icon, tray menu, close-to-tray, single instance, and optional Windows autostart | [L1], [L2] | Strong match for the core B+C+D category; most verified peers are macOS-first or CLI/dashboard-first. |
| Codex | Live quota via local Codex login; backend usage request; normalized windows | [L1], [L3] | Differentiated combination: Codex quota and reset data in a Windows tray. |
| Z.ai | Live quota via local ZCode/API key candidates; normalized windows | [L1], [L3] | Rare provider coverage in the verified set. |
| OpenCode Go | Live quota via local OpenCode auth key; 5-hour/weekly/30-day windows | [L1], [L3] | Rare provider coverage; more specific than generic token counters. |
| Antigravity | Passive local plugin quota cache, source age retained and classified fresh/stale over 24 hours | [L1], [L3] | Freshness-aware cached quota is a useful trust pattern, not just another provider card. |
| Last-good retention | Provider card keeps last good windows on refresh failure; red status and tooltip explain failure | [L1], [L2], [L3] | Directly comparable to TokenBar's last-known-value behavior and stronger than blanking. |
| Structured failures | `{ code, message }` errors for missing login, rejected credentials, network, and schema changes; deterministic failures are not retried | [L2], [L3] | Trust/transparency advantage when surfaced clearly in UI. |
| Reset timestamps | RFC 3339 `resetAt`; Codex fallback from `reset_after_seconds`; no reset invented when missing | [L1], [L3] | The data model is ready for a countdown even though the current UI does not show one. |
| Configurable refresh | 1/5/15/30 minutes; refresh coalescing; refresh on stale reopen | [L1], [L2] | Commodity control, but the cadence/staleness relationship is a useful differentiator. |
| Local-first credential handling | Read-only local credential stores in Rust; WebView receives normalized windows only; no credential logging | [L1], [L3] | Safer and more explicit than broad credential upload models. |
| Installer | NSIS installer produced by Tauri; unsigned and subject to SmartScreen warnings | [L1], [L4] | Distribution readiness is ahead of many source-only utilities; signing remains a trust gap. |
| CI | Windows GitHub Actions workflow: offline Rust tests, Vitest, Tauri build, installer upload; live credential tests excluded | [L4] | Good reproducibility story for a Windows-first utility. |
| Historical usage | No database; quota data is in memory only | [L1] | A clear scope boundary versus TokenTracker/ccusage/Syrtis history features. |
| Cost tracking | Not present | [L1] | Keeps Rate Limits focused on quota/reset; cost analytics is an adjacent market. |
| Prediction/projection | Not present | [L1] | Candidate v0.3 experiment, not a commodity requirement. |
| Multiple quota windows | Multiple normalized windows per provider, with per-window labels and reset times | [L1], [L3] | Core value: compare 5-hour, weekly, and longer windows without provider-specific UI. |

## Candidate differentiators

| Candidate | Evidence from market | Evaluation | Disposition | Destination |
|---|---|---|---|---|
| Reset countdown | TokenTracker advertises Limits-page countdowns; QuotaBar/QuotaBar advertises reset times; Rate Limits already carries exact `resetAt` values | High-value, low-ambiguity addition to existing data | **ADOPT** | Rate Limits v0.3 |
| Prediction / burn-rate | TokenBar advertises pace projections; ccusage has `blocks --live` burn rate | Promising but easy to overclaim when provider quota semantics differ | **TEST** | Rate Limits v0.3 experiment with explicit confidence/assumptions |
| Stale-aware quota | TokenBar retains last-known values; Rate Limits already has last-good and Antigravity freshness metadata | Should become a visible, provider-wide trust model rather than a footer-only detail | **ADOPT** | Rate Limits v0.3 |
| Windows-first multi-provider experience | Verified peers skew macOS or CLI; Rate Limits already combines Windows tray, Codex, Z.ai, and OpenCode Go | Most defensible positioning if provider contracts remain reliable | **ADOPT** | Rate Limits v0.3 positioning and acceptance criteria |
| Themes / glass UX | TokenBar uses Liquid Glass and a 3D graph; QuotaBar uses compact meters | Themes may improve accessibility and personalization; glass is decorative and platform-specific | **TEST** for accessible themes; **IGNORE** for glass effects | Theme experiment in v0.3; no glass dependency |

## RADAR

| Pattern | What the market shows | Disposition | Destination |
|---|---|---|---|
| Reset countdown | Users need to know when a window becomes available, not only the percentage used | **ADOPT** | Rate Limits v0.3 |
| Burn-rate / projection | TokenBar pace cards and ccusage live blocks make consumption trajectory visible | **TEST** | v0.3 experiment behind assumptions and a confidence label |
| Last-known-value retention | TokenBar explicitly preserves the previous reading after refresh failure | **ADOPT** | v0.3 provider status model |
| Freshness badges and source age | Rate Limits Antigravity cache already distinguishes fresh, stale, and indeterminate data | **ADOPT** | v0.3 across all provider cards |
| Windows tray-first multi-provider cockpit | The verified field is dominated by macOS menu bars and local dashboards | **ADOPT** | v0.3 product positioning and Windows acceptance tests |
| Glanceable menu title / icon modes | TokenBar offers signal bars, rings, and other title modes; QuotaBar is built around a compact meter | **WATCH** | Backlog only; not part of v0.3 |
| Local transcript cost analytics | TokenTracker, ccusage, and Syrtis make local log parsing and estimated cost a primary workflow | **WATCH** | Backlog only if users ask for spend visibility beside quota |
| Historical charts and heatmaps | TokenTracker and Syrtis use history to explain usage patterns | **WATCH** | Backlog; needs a retention and privacy decision first |
| Account switching | chilohwei/QuotaBar makes local account switching a core action | **WATCH** | Backlog after multi-account demand is proven |
| Broad provider catalog | QuotaBar/QuotaBar advertises 22 providers; TokenTracker advertises 31 tools | **WATCH** | Add providers only after contract/error tests are stable |
| Mobile companion | CodexBar exposes limits, spend, tokens, and budgets on iOS | **IGNORE** | Out of scope for the Windows tray product |
| 3D contribution graph and mascot motion | Syrtis uses a 3D graph and RunCat-style motion | **IGNORE** | Do not copy decorative novelty into v0.3 |
| Cloud accounts and telemetry | TokenTracker explicitly offers local-first/no-telemetry operation | **IGNORE** | Keep local-first credential and quota handling |
| Pricing catalog | TokenTracker uses LiteLLM pricing and badges missing prices | **IGNORE** unless cost tracking becomes a committed goal | Keep out of the quota/reset core |
| Browser dashboard | TokenTracker's active experience is a local browser dashboard | **WATCH** | Consider only as a companion surface after tray UX is proven |

## Sources

### Current web/product evidence

- **[S1]** [xiufengsun/TokenTracker](https://github.com/xiufengsun/TokenTracker) and [Token Tracker pricing](https://www.tokentracker.cc/pricing), retrieved 2026-09-27. Primary/vendor claims: 31 tools, local-first usage/cost tracking, provider quota checks, native apps, prompt privacy, optional telemetry.
- **[S2]** [Token Tracker product site](https://www.tokentracker.cc/), retrieved 2026-09-27. Secondary support for local dashboard and `npx tokentracker-cli` flow.
- **[S3]** [Nanako0129/TokenBar](https://github.com/Nanako0129/TokenBar), retrieved 2026-09-27. Primary/vendor claims: native Swift macOS menu bar, 25+ agents, quota cards, local session logs, Liquid Glass, 3D contribution graph.
- **[S4]** [TokenBar official site](https://tokenbar.nyanako.com/), retrieved 2026-09-27. Primary/vendor claims: macOS requirements, OAuth quota lookups, no telemetry/account, Homebrew install, last-known-value behavior.
- **[S5]** [chilohwei/QuotaBar README](https://github.com/chilohwei/QuotaBar/blob/main/README.en.md), retrieved 2026-09-27. Primary/vendor claims: Claude Code/Codex/Cursor quota, local config read/write, official API refresh, account switching.
- **[S6]** [QuotaBar/QuotaBar](https://github.com/QuotaBar/QuotaBar), retrieved 2026-09-27. Primary/vendor claims: macOS menu-bar meter, 22 providers, quota usage, reset times, locally estimated costs.
- **[S7]** [quota.bar](https://quota.bar/), retrieved 2026-09-27. Primary/vendor positioning for QuotaBar/QuotaBar.
- **[S8]** [ryoppippi/ccusage](https://github.com/ryoppippi/ccusage), identity and core scope referenced in the current research pass. Primary scope: local Claude Code JSONL analysis; fine-grained current README details are marked UNVERIFIED where not independently retrieved.
- **[S9]** [ccassist.dev](https://ccassist.dev), secondary comparison retrieved 2026-09-27. Secondary claims: ccusage is terminal-native and focused on local usage/cost analysis.
- **[S10]** [tokn.watch](https://tokn.watch), secondary comparison dated 2026-09-18. Secondary claims: ccusage is open source/free, `npx` usage, and approximately 18,000 GitHub stars.
- **[S11]** [syrtis.nyanako.com](https://syrtis.nyanako.com), retrieved 2026-09-27. Primary/vendor claim: TokenBar was renamed to Syrtis at v2.0 and upgrade preserves preferences/history.
- **[S12]** [pitimon/TokenTracker](https://github.com/pitimon/tokentracker), retrieved in delegated research 2026-09-27. Primary/vendor claims: cross-platform CLI/local dashboard, quota chips and Limits-page reset countdowns, MIT, LiteLLM pricing, local-first operation. Kept separate from S1 because the name is ambiguous.
- **[S13]** [devtrends.site](https://devtrends.site), secondary snippet dated 2026-09-16. Secondary lead for gentpan/QuotaBar; not used as primary evidence.
- **[S14]** [Juejin](https://juejin.cn) and [Gitblind](https://gitblind.noratr.app), secondary snippets dated 2026-08-31. Secondary lead for `majiayu000/quotabar`; not used as primary evidence.
- **[S15]** [CodexBar App Store listing](https://apps.apple.com/), current product listing surfaced in research on 2026-09-27. Vendor claims: rate limits, spending, token usage, budget tracking on iOS.
- **[S16]** [poweredbyai.app](https://poweredbyai.app), secondary roundup surfaced in delegated research on 2026-09-27. Leads for cc-hdrm, Usage4Claude, SessionWatcher, and claude-spend; all fine-grained fields remain UNVERIFIED.

### Local Rate Limits evidence

- **[L1]** [README.md](C:/AI/Token_Monitor/README.md), current checkout. Product baseline, provider status, tray/window behavior, refresh, stale indicator, local credential boundaries, and installer notes.
- **[L2]** [docs/release-notes-v0.2.0.md](C:/AI/Token_Monitor/docs/release-notes-v0.2.0.md), current checkout. v0.2.0 feature and validation evidence.
- **[L3]** [docs/provider-contract-audit.md](C:/AI/Token_Monitor/docs/provider-contract-audit.md), current checkout. Provider normalization, reset semantics, last-good retention, structured errors, freshness, and credential redaction.
- **[L4]** [docs/ci-and-release.md](C:/AI/Token_Monitor/docs/ci-and-release.md) and [.github/workflows/ci.yml](C:/AI/Token_Monitor/.github/workflows/ci.yml), current checkout. Windows CI stages and unsigned NSIS installer artifact.

## Reusable multi-model review brief

Use this brief later with multiple independent reviewers. Do not run the review as part of this benchmark.

- **Review target:** `docs/market-benchmark.md` and the linked product evidence.
- **Constraint:** review the research artifact and recommendations only; do not modify application code.
- **Evidence rule:** separate primary/vendor claims, secondary claims, and UNVERIFIED fields. Do not turn absence of evidence into a negative feature claim.

Ask each reviewer to independently critique:

1. **Information hierarchy:** can a reader find the market shape, evidence strength, and recommendation without reading every row?
2. **Clarity:** are category boundaries A-E, product identities, and the Rate Limits baseline unambiguous?
3. **Usefulness:** do the recommendations lead to concrete v0.3 decisions?
4. **Trust/transparency:** are source attribution, vendor claims, unresolved names, and verification gaps honest and prominent?
5. **Visual density:** is the wide benchmark table scannable, and are any fields too ambiguous or redundant?
6. **Windows tray ergonomics:** would the proposed status, reset, stale, and failure cues work at a glance in the Windows tray?
7. **Prediction UX:** does the proposed burn-rate/projection experiment explain assumptions, confidence, and failure behavior?
8. **Onboarding:** can a new user understand provider prerequisites, local credential handling, and recovery steps?
9. **Edge cases:** are missing reset timestamps, stale caches, schema changes, expired credentials, offline starts, and partial provider failures covered?
10. **Differentiation:** are the recommended patterns genuinely distinct from commodity cost trackers and macOS menu-bar utilities?

Each reviewer should return: (a) the five highest-impact issues, (b) one concrete revision per issue, (c) any claim that needs a stronger source, and (d) a short answer to the five final-assessment questions. Do not produce overall product scores or rankings.

## Final assessment

### 1. What capabilities are already commodity?

- Local log parsing and estimated token cost are established patterns in TokenTracker, ccusage, and Syrtis. **Disposition: IGNORE** as a differentiator for Rate Limits v0.3.
- Claude/Codex quota cards and basic reset display appear across the macOS menu-bar tools. **Disposition: WATCH**; implement only where the provider contract is verified.
- A menu-bar/tray presence is standard for the category. **Disposition: ADOPT** as table stakes for the Windows experience.
- Historical charts and cost tables are common in analytics-oriented tools. **Disposition: WATCH** outside the current in-memory scope.

### 2. Which 3 product patterns are worth adopting?

1. **Reset countdown.** Show time remaining and the exact reset timestamp together. **Disposition: ADOPT** to **Rate Limits v0.3**.
2. **Stale-aware quota with last-known-value retention.** Preserve data, show source age, and distinguish live, cached, stale, and failed states. **Disposition: ADOPT** to **Rate Limits v0.3**.
3. **Windows-first multi-provider tray ergonomics.** Make Codex, Z.ai, OpenCode Go, and Antigravity comparable at a glance with provider-specific failure cues. **Disposition: ADOPT** to **Rate Limits v0.3**.

### 3. What should we explicitly avoid building?

- A full API billing ledger or 2,200-model pricing engine. **Disposition: IGNORE**.
- Cloud accounts, sync, or telemetry for quota data. **Disposition: IGNORE**.
- A mobile companion app. **Disposition: IGNORE**.
- Decorative 3D graphs, mascots, or glass effects as core UI. **Disposition: IGNORE**.
- Credential mutation or automatic OAuth refresh. **Disposition: IGNORE**; retain read-only local credential handling.

### 4. What could genuinely differentiate Rate Limits?

- A **Windows-first, multi-provider quota cockpit** that covers Codex, Z.ai, OpenCode Go, and cached Antigravity in one normalized tray model. **Disposition: ADOPT**.
- **Trustworthy freshness and failure semantics**: exact reset timestamps, visible source age, last-good retention, and structured errors instead of blank or invented values. **Disposition: ADOPT**.
- A **safe local credential boundary** with read-only access, no WebView secret exposure, and explicit recovery instructions. **Disposition: ADOPT**.

### 5. What should be tested in v0.3 rather than immediately adopted?

- Burn-rate and projection UX with explicit assumptions and confidence. **Disposition: TEST**.
- Accessible themes. **Disposition: TEST**. Alternate tray-title/icon modes remain **WATCH** and are outside v0.3.
- Historical retention and simple trend views after deciding what is stored locally. **Disposition: TEST**.
- Glass-inspired visual treatment only as an optional skin, never as the information model. **Disposition: TEST**.
