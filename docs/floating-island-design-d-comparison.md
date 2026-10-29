# Floating Island — comparison with the accepted "D — Complete Floating Island" design

Baseline: the accepted pen.dev document
`untitled.pen`, frame `LzGx8`, rows D1–D8, against the implementation on
`origin/main` @ `32397dd3` plus the multi-account work.

The design's coordinates are shell-relative (the bar shell is a child of
the window frame), so the comparison below adds the 4 px window frame where
the implementation is measured in window coordinates.

Every number below is measured in the running app (the dev floating
surface, `?surface=floating&fixture=…`) and compared with the exported
design frames in `artifacts/design-d/`.

## Geometry

| Row | Design | Implementation | Verdict |
| --- | --- | --- | --- |
| D1 five providers | window 600×64, shell 592×56, segments 112×46 at shell 19/133/247/361/475, meters 62×3 at 40 within the segment | window 600×64, shell 592×56, segments 112 at window 23/137/251/365/479 (= shell 19/133/247/361/475), meters 62×3 at window 63/177/291/405/519 | identical |
| D2 shared wallet slot | window 646×64 = 600 + 44 + 2, slot 44 at shell 589 (window 593) | window 646×64, slot 44 at window 593 | identical |
| D3 resting dock | 600×16, meters 62×3 at 63/177/291/405/519, separators on the segment boundaries | dock state `resting`, height 16, tracks 62×3 at 63/177/291/405/519 | identical |
| D4 provider card | card frame 300 wide, under its own segment | popover frame 300 wide, clamped to the shell | identical |
| D6 two accounts | window 646×64, six segments of 93 (pitch 95), wallet slot at 593 | window 646×64, six segments of 93 (pitch 95), wallet slot at 593 | identical |
| D8 dynamic width | clamp(40 + n × 120, 300, 600); 4 providers → 520 with ≈120.5 segments; ≥7 → compact ≈79 | 520 at four providers (120.5 per segment), 600 at five, compact ≈79 at seven | identical |

The design's own claim — "3 px meters sit exactly on the revealed bar's
meter positions, so the reveal never slides them" — is verified numerically:
the resting dock's tracks and the revealed bar's meters share the same
left offsets and the same 62 px width.

## States (D7)

| State | Design | Implementation |
| --- | --- | --- |
| Stale | 44% with an amber dot | 44% with an amber dot |
| Loading | em dash, empty track | em dash, empty track |
| Unavailable | em dash, empty track, "Unavailable" chip on the card | em dash, empty track, "Unavailable" chip on the card |
| Expired session | "!" badge over the last known value | "!" badge over the last known value |
| Healthy | no badge | no badge |

The "!" badge marks a degraded READING, so it is drawn only over a value.
The implementation now follows that rule: a provider with no reading at all
shows a bare em dash and the expanded card states the reason. The spoken
label still announces the error.

## The accepted correction (D6 alias chips)

The design frames D6 carry an `M` and a `W` chip beside the two account
percentages. Human Acceptance removes them, so the compact bar carries no
account marker of any kind: no initial, no letter, no numeric account id
and no badge. Both account segments show the provider mark and the
percentage only.

The account is named where the detail is shown: the expanded card's header
carries the alias (`OpenAI / Codex · Work — WG`) and its footer carries the
masked identity (`account ••7d10b6e8`). The spoken label names the account
too, so two accounts of one provider are distinguishable without a visible
badge.

## Screenshots

| File | Shows |
| --- | --- |
| `artifacts/screenshots/01-floating-bar-two-accounts.png` | D6 — two accounts, six segments, shared wallet slot |
| `artifacts/screenshots/02-floating-bar-five-providers.png` | D1 — five providers, 600 px window |
| `artifacts/screenshots/03-account-popover-secondary.png` | the expanded card of the second account |
| `artifacts/screenshots/04-resting-dock.png` | D3 — the resting dock |
| `artifacts/screenshots/05-representative-states.png` | D7 — stale, loading, unavailable, expired, healthy |
| `artifacts/screenshots/06-paused-account.png` | D6 after-pause — the frozen account segment |
| `artifacts/screenshots/07-settings-codex-accounts.png` | the account configuration in Settings |

