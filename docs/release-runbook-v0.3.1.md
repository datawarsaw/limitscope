# Rate Limits v0.3.1 — Release Runbook

The standing, repeatable process for cutting the v0.3.1 release — and the template for later
patch releases. Policy background lives in [ci-and-release.md](ci-and-release.md): CI never
signs, publishes, tags, or releases; the release itself is a manual, gated process.

Companions:

- Acceptance checklist: [human-acceptance-v0.3.1.md](human-acceptance-v0.3.1.md) (SHA-bound;
  `latest` or a branch name alone is never accepted)
- Acceptance helper: [../scripts/human-acceptance-v03.ps1](../scripts/human-acceptance-v03.ps1)
  (run with `-AppVersion 0.3.1`; report defaults to the git-ignored `artifacts/` directory)

## Lessons from v0.3.0 → mechanisms in this runbook

| # | v0.3.0 lesson | Mechanism here |
|---|---|---|
| 1 | A pre-flight run caught a stale v0.2.0 binary being tested instead of the v0.3.0 build | Checklist section 1 pre-flight: deterministic file/product version comparison (`-AppVersion` guard in the helper) + SHA-256, never the filename |
| 2 | Acceptance evidence and the tested SHA must be tied to the exact RC commit | Checklist section 1 identity binding and section 9 result block; runbook gate G1 |
| 3 | The execution environment may lack a native Computer Use surface; a bounded PowerShell acceptance helper is useful | `scripts/human-acceptance-v03.ps1`: deterministic checks only (metadata, hashes, process lifecycle); every tray/UI judgment stays with the operator |
| 4 | Post-merge CI must pass on the exact merge SHA before tagging | Gate G3 |
| 5 | The final installer artifact must be verified against the exact successful CI run | Stage 5: artifact downloaded from the post-merge run on the merge SHA, hash-compared (gate G4) |
| 6 | Installer filename, size, SHA-256, and version metadata must be recorded | Stage 5 recording table and the release record at the end of this file |
| 7 | The tag must be annotated and resolve to the exact release commit | Stage 6: annotated tag on the verified merge SHA, resolved and re-checked (gate G5) |
| 8 | GitHub Release only after acceptance PASS, merge, post-merge CI PASS, and artifact verification | Gate G6 preconditions, all four checked before `gh release create` |
| 9 | GitHub may normalize release asset display names; integrity is determined by content/hash, not display-name punctuation | Stage 7 verification compares the uploaded asset's digest/size, never its display name |
| 10 | Authenticated browser tooling was unreliable for final release publication; local git/gh delivery is the accepted path | Stage 7 delivery is local git + GitHub CLI by default |

## Lifecycle

```text
candidate branch → CI green → Human Acceptance → merge → post-merge CI
  → artifact verification → annotated tag → GitHub Release → final verification → Done/Closed
```

Every arrow is a numbered stage below with its STOP gates. No stage may be skipped, and any
STOP sends the release back to the stage named in the gate — never forward.

## Stage 1 — Candidate branch and CI green

- Cut the candidate branch from `main` (or the current release line) and open a PR.
- The version bump to `0.3.1` (`package.json`, `package-lock.json`,
  `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`) lands on the
  candidate branch **before** the RC build, so CI artifacts carry `0.3.1` metadata.
- **STOP gate G0:** PR CI must be green on the exact candidate head SHA. Record the run ID
  and head SHA. A superseded run (the workflow cancels superseded runs on the same ref) is
  not evidence.

## Stage 2 — Human Acceptance (pre-merge)

- Run [human-acceptance-v0.3.1.md](human-acceptance-v0.3.1.md) against the candidate build.
- **STOP gate G1:** if the checklist's tested SHA ≠ the candidate head SHA, **STOP**. A moved
  candidate head invalidates the run: re-cut, re-run acceptance on the new SHA.
- **STOP gate G2:** outcome must be `PASS` or `PASS WITH NON-BLOCKING NOTES`. Any `FAIL` or
  checklist `STOP` = **STOP** (one bounded corrective task, then re-run acceptance).

## Stage 3 — Merge

- Merge the PR and immediately record the merge SHA:
  `git fetch origin && git rev-parse origin/main`.
- Sanity bridge between the tested build and the tagged build:
  `git rev-parse <merge-sha>^{tree}` must equal
  `git rev-parse <candidate-sha>^{tree}`.
- **STOP gate G2b:** if the trees differ, unexpected content entered the merge — **STOP**
  and re-run Human Acceptance against the new tree before continuing.

## Stage 4 — Post-merge CI

- Wait for CI on `main` and record run ID, conclusion, and head SHA.
- **STOP gate G3:** if the post-merge CI head SHA ≠ the merge SHA, **STOP** (a newer commit
  landed; re-verify against the correct run or repeat from Stage 3).
- Conclusion must be `success`. A cancelled or stale run is not evidence.

## Stage 5 — Artifact verification

- Download artifact `rate-limits-windows-installer` from the **post-merge** run
  (Stage 4 run ID) via `gh run download <run-id> -n rate-limits-windows-installer -D <dir>`
  or the Actions UI. Record the artifact ID/name.
- Verify and record — this is the release artifact:

| Field | Value |
|---|---|
| Workflow run ID | `<run id>` |
| Workflow conclusion | `success` |
| Workflow head SHA | `<merge sha>` |
| Artifact ID / name | `<id>` / `rate-limits-windows-installer` |
| Installer filename | `Rate Limits_0.3.1_x64-setup.exe` |
| Installer size (bytes) | `<size>` |
| Installer SHA-256 | `<sha-256>` |
| PE/NSIS sanity | valid PE header; NSIS stub machine `0x14C` (32-bit launcher) is expected |
| FileVersion | `0.3.1` |
| ProductVersion | `0.3.1` |
| ProductName | `Rate Limits` |

- `Get-FileHash`, file size, and version metadata can be read with the helper in
  installer-only mode:
  `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\human-acceptance-v03.ps1 -InstallerPath <installer> -AppVersion 0.3.1`
- **STOP gate G4:** if the installer SHA-256 differs from the hash recorded for the verified
  CI artifact, **STOP** — never install, attach, or publish an unverified binary.

## Stage 6 — Annotated tag

- Check first: `git rev-parse -q --verify refs/tags/v0.3.1` must not resolve to another
  commit. **STOP gate G5:** if the tag already exists at a different commit, **STOP**.
  Release tags are never force-moved (`git push -f` on a release tag is prohibited); a
  mistaken tag requires an explicit operator decision recorded in the release record before
  anything else happens.
- Create and push an annotated tag on the verified merge SHA:
  `git tag -a v0.3.1 -m "Rate Limits v0.3.1" <merge-sha> && git push origin v0.3.1`
- Verify: `git cat-file -t v0.3.1` → `tag` (annotated), and
  `git rev-parse v0.3.1^{}` → the merge SHA.

## Stage 7 — GitHub Release (local git + gh CLI)

Preferred delivery is local git plus the GitHub CLI (lesson 10); browser-based publication is
the fallback, not the default. Conceptual sequence — placeholders only, no future SHA is
ever hardcoded:

```text
git fetch origin --tags --prune          # verify refs: origin/main == merge SHA, tag state
gh run download <run-id> -n rate-limits-windows-installer -D <dir>   # from the Stage 4 run
# hash the installer; gate G4 must pass on this exact copy
git tag -a v0.3.1 -m "Rate Limits v0.3.1" <merge-sha>               # gate G5 checked first
git push origin v0.3.1
gh release create v0.3.1 "<installer-path>" --title "Rate Limits v0.3.1" --notes-file <notes>
# verify the release (next section)
```

- **STOP gate G6:** all four preconditions must be recorded before `gh release create`:
  Human Acceptance PASS (tested SHA = candidate SHA, trees bridged per G2b), merge complete
  (merge SHA recorded), post-merge CI PASS on the merge SHA (G3), artifact verification PASS
  on this exact copy (G4). Missing any one = **STOP**.

## Stage 8 — Final verification → Done/Closed

- `gh release view v0.3.1` shows the installer asset; compare the uploaded asset's
  **digest/size** against the Stage 5 values — GitHub may normalize the display name
  (lesson 9), so match by content, never by display-name punctuation.
- Re-download the published asset and re-hash if any doubt remains.
- The tag on `origin` resolves to the merge SHA (Stage 6 checks).
- Fill the release record below and mark the release **Done/Closed**.

## Release record (v0.3.1 - Done/Closed, 2026-09-28)

| Field | Value |
|---|---|
| Candidate branch / SHA | `release/v0.3.1-rc` / `15e95e4de7ba7313ff217d7202c766c5f0ea32bd` (PR #4 head; product tree unchanged from the tested `33899fc`) |
| Tested commit SHA (checklist section 9) | `33899fc9baa3eb149625c57f75d833d13b80e06e` |
| Merge SHA | `6da76488871fc8924a06154142a2552b34057c18` |
| Post-merge CI run ID / conclusion / head SHA | `36472269831` / `success` / `6da76488871fc8924a06154142a2552b34057c18` |
| Artifact ID / name | `10992377211` / `rate-limits-windows-installer` |
| Installer filename / size / SHA-256 | `Rate Limits_0.3.1_x64-setup.exe` / 1558450 / `1C87EC17193A34E14E15F23ED045DC0D2C6E83C3552AE044DB86827CFC81B049` |
| FileVersion / ProductVersion / ProductName | `0.3.1` / `0.3.1` / `Rate Limits` |
| Tag / tag commit | `v0.3.1` (annotated, tag object `eaf42cb010e3abd361be847fec5d6527df0fa5e8`) / `6da76488871fc8924a06154142a2552b34057c18` |
| Release URL / asset digest match | https://github.com/datawarsaw/rate-limits/releases/tag/v0.3.1 / yes |
| Result | Done/Closed |

Gates: G1/G2 (tested SHA = candidate SHA, PASS) satisfied; G2b tree bridge satisfied - the merge tree
`829c67cc1ba7ef97b33c3beaf8d04ea878b02cff` equals the record-commit tree, and differs from the tested
`33899fc` tree only in `docs/human-acceptance-v0.3.1.md` and `state/project-state.md`; G3 satisfied
(post-merge CI success on the exact merge SHA); G4 satisfied (published asset re-downloaded and
re-hashed to the recorded SHA-256); G5 satisfied (no pre-existing `v0.3.1` tag); G6 satisfied (all four
preconditions recorded before publication). GitHub normalized the published asset name to
`Rate.Limits_0.3.1_x64-setup.exe`; integrity was matched by content, per lesson 9.
