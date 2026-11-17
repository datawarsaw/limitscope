# npm dependency remediation for 0.8.24

Local candidate only. Release remains HOLD; no vulnerability is accepted by
this document. Audit results are time-dependent and count affected packages,
not individual advisories.

## Policy and compatible changes

`package.json` permits install scripts only for `esbuild@0.21.5`. npm 12's
default policy reports unreviewed scripts rather than blocking them: use
`npm ci --strict-allow-scripts` to enforce that policy. No other package is
approved. Reassess the pin whenever esbuild changes.

The lockfile updates `source-map-js` 1.2.1 to 1.2.2 within the `^1.2.1`
ranges of postcss and css-tree, and `undici` 8.11.2 to 8.12.3 within
jsdom's `^8.10.2` range. These resolve the corresponding high advisories;
no direct dependency range or override changed. A clean npm 12.0.2 install
with `NODE_USE_SYSTEM_CA=1`, strict TLS and a dedicated cache ran only the
esbuild postinstall. `esbuild --version` returned 0.21.5; no scripts were
pending review. Frontend tests (1474 in 85 files), TypeScript/Vite build,
`npm ls --all` and `git diff --check` passed.

## Remaining audit findings and exposure

On 2026-10-30, npm audit changed from 8 affected dev packages (2 critical,
3 high, 3 moderate) to 6 (2 critical, 1 high, 3 moderate). The remaining
graph and maximum severities are:

- `vitest@2.1.9` (critical): the UI server file read/execution issue is
  fixed in 3.2.6 or 4.1.0; the project's `vitest run` does not start the
  UI, and `@vitest/ui` is not installed. UI or browser mode on Windows, or
  an exposed UI server, changes that exposure.
- `tinypool@1.1.1` (critical), via vitest `^1.0.1`: the two prototype
  pollution execution gadgets require a separate pollution primitive;
  both are fixed in 2.1.2, outside the permitted range. Test workers execute
  during normal tests, so reachability is not ruled out.
- `vite@5.4.21` (high): Windows editor-launch command injection requires
  attacker control of the editor file argument; dev server features are
  relevant when `npm run dev` is used. The project does not configure a
  network host. Other Vite issues affect dev server paths and middleware;
  fixes require a newer major. Production build uses Vite, but the advisories
  do not establish that the shipped frontend exposes the dev server.
- `esbuild@0.21.5` (moderate), via Vite `^0.21.3`: its server issue is
  relevant if esbuild's own development server is used; the configured build
  uses esbuild for minification, not its server. Fixed in 0.25.0, outside
  Vite's range.
- `@vitest/mocker@2.1.9` and `vite-node@2.1.9` (moderate), via Vitest:
  mocker has redirect mock and manual mock registration advisories; test
  files use mocks. Exploit prerequisites and reachability have not been
  fully established. vite-node inherits the Vite finding.

Sources: [Vitest UI](https://github.com/advisories/GHSA-5xrq-8626-4rwp),
[tinypool worker options](https://github.com/advisories/GHSA-5gmw-xhrv-c9v3),
[tinypool run options](https://github.com/advisories/GHSA-85c8-ppgw-ccpr),
[Vite Windows editor](https://github.com/advisories/GHSA-587h-7wmj-h5pm),
[esbuild server](https://github.com/advisories/GHSA-67mh-4wv8-2f99),
[mocker redirect](https://github.com/advisories/GHSA-82fw-gwwq-j7x9),
[mocker registration](https://github.com/advisories/GHSA-qxg6-89mg-rqv8).

## Proposed separate upgrade

Evaluate a compatible patched Vite/Vitest pair and `@vitejs/plugin-react`
peer ranges and Node requirements together. Check all current advisory ranges,
including tinypool and mocker, rather than assuming one major resolves all
findings. In a separate bounded change, update direct dependencies and lockfile,
review the new install-script set, run a strict clean install, audit, full
frontend tests and production build, then validate Tauri dev and native flows.
Review regressions and unresolved findings independently before Human
Acceptance or any release decision. npm audit suggested Vite 8.3.5 and Vitest
5.0.5; those are suggestions, not a tested or approved upgrade plan.
