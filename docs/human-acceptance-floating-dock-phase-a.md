# LimitScope Floating Bar — Grok Visibility, Dynamic Resting Dock, Wallet Dot
— Human Acceptance

Candidate worksheet for the bounded Phase A Floating Bar UX change. The
accepted Halo visual language, meter styling, motion and monitor behavior are
unchanged; this candidate changes what the floating bar SHOWS (Grok's
runtime observation), how WIDE the resting dock is, and adds one wallet
status dot.

## 1. Candidate

| Item | Value |
|---|---|
| Branch | `codex/floating-dock-phase-a` |
| Base | `9ac6fe7` (v0.8.15 publication evidence, `origin/main`) |
| Commit | `e141c93248b07594faaba92800f66b37bac65149` |
| Scope | Frontend only (no Rust change, no version bump, no release) |

## 2. What changed

| Change | Before | After |
|---|---|---|
| Grok in the floating bar | Only the manual UIA read; started empty and said "No Grok Bot data yet" | The runtime's passive Grok Bot observation — the same one the main dashboard renders — with the manual read as explicit fallback |
| Resting dock width | Fixed 600 px, so a narrower visible set left empty desktop width | The active bar's width for the same visible providers (and the same wallet slot) |
| Resting ↔ revealed meters | Resting meters offset by half the width difference to survive the re-centering reveal | Resting meters sit exactly on the revealed bar's meters; no offset |
| Wallet in the resting dock | Absent | One status dot in the revealed bar's own wallet slot |

## 3. Checks

### Grok visibility

- [ ] With a live Grok observation (Grok Bot · X Premium+), open the floating
      bar's Grok card. It shows the plan label, the remaining percentage, and
      the reset — the same pool the main window's Grok card shows.
- [ ] The floating Grok card and the main window's Grok card show the SAME
      percentage. They never disagree.
- [ ] The Grok segment's own number is still the xAI weekly-credit window, not
      the Grok Bot pool. The two are never summed or merged.
- [ ] With no observation, the card shows the manual read's guidance and the
      Refresh action still works.
- [ ] After a manual Refresh, the passive observation still wins — the
      manual read never displaces it.

### Dynamic resting dock

- [ ] Dock to the top edge with three visible providers. The resting strip
      measures 400×16 logical px at 100% scaling — no empty width on the
      right.
- [ ] The strip contains exactly three 3px meters and two separators.
- [ ] Hovering reveals a 400×64 bar. The top edge stays fixed and the
      horizontal center does not move.
- [ ] Each resting meter crossfades into the revealed meter at the same x and
      width. No meter slides horizontally during the reveal.
- [ ] With five visible providers, resting and revealed are both 600 px.
- [ ] With a wallet present, resting and revealed are both 646 px (five
      providers plus the wallet slot).
- [ ] Hide a provider in Settings while resting: the native window and the
      strip shrink together (for example 400 → 300 for 3 → 2), the meters
      reflow, and the right edge follows the new width.
- [ ] Show the provider again: the strip returns to the wider footprint.
- [ ] Hide or show a provider while revealed: the bar width changes in place,
      stays revealed, and keeps its top edge and center.
- [ ] With a pinned card open, hide or show another provider: the card stays
      open, the bar and window reflow, and closing the card settles to the new
      resting width.
- [ ] The 16px strip remains the whole hover and click target.

### Wallet status dot

- [ ] With OpenRouter present, the resting strip shows one small dot in the
      same slot the revealed bar's wallet occupies — to the right of the last
      provider.
- [ ] The dot's color matches the revealed wallet slot for the same state:
      live green, stale/cooldown amber, error red, unavailable muted.
- [ ] The dot NEVER shows a percentage, a filled meter, or a quota color. A
      wallet is an absolute money balance, not a quota window.
- [ ] With no wallet, the strip shows no dot.

### Preserved behavior

- [ ] Semantic meter colors, 3px meter height, separators, surface, radius
      and reveal timing are visually unchanged.
- [ ] Drag, drag-away undock, saved anchor, menu actions and reduced-motion
      behavior are unchanged.
- [ ] The main dashboard's Grok card, provider rail and quota windows are
      unchanged.
- [ ] No second window, no new layout system, no provider collector or
      refresh change, and no API Wallets credential change.

## 4. Automated evidence

- Targeted Vitest: `floatingQuota.test.ts`, `floating.css.test.ts`,
  `FloatingQuotaWindow.test.tsx`, `FloatingQuotaBar.test.tsx`,
  `GrokBotSection.test.tsx`, `floatingWindowChrome.test.ts` — all pass.
- Full frontend Vitest: 1183/1183 passed (1169 before this candidate,
  +14 new).
- `tsc --noEmit`: PASS.
- `npm run build` (TypeScript plus Vite): PASS.
- `npm run scan:secrets`: PASS.
- `cargo check --locked --all-targets`: PASS (3 pre-existing dead-code
  warnings in unrelated modules; no Rust change in this candidate).

Manual Windows/Tauri interaction was NOT performed in this run. The checks
above remain required for Human Acceptance. The native geometry claims
(400×16, 646×16, meter x positions) are pinned by automated tests against the
same layout constants the CSS uses, but the rendered result still needs a
human eye.

## 5. Out of scope

- Provider collectors and quota refresh logic.
- Wallet credential handling.
- DeepSeek integration, monitor recovery.
- Version bump, tag, merge, publication or release.
- The pending 7-day reset-horizon candidate (`9879ee7`) — a separate
  candidate that remains unmerged.
