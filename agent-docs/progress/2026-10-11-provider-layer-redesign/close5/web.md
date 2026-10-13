# close5 / web

tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]

- Routing catalog の deployment の lane 表示を、旧 tier alias の接頭辞（`celeris/<lane>`）なしの lane 名そのもの（例: `cheap`、`frontier, standard`）にした。変更: `web/features/ops/routing-catalog-section.tsx`。
- `web/features/ops/routing-catalog.test.tsx` の期待を lane 名に替えた。`lane: </dt><dd class="inline"><span>cheap</span>` と `frontier, standard` を期待し、出力に `celeris/` が含まれないことを確かめる（コメントの接頭辞の語も外した）。
- fixture（`web/e2e/support/fake-daemon.mjs`）は変えていない。crates/ も変えていない。

証拠（この WU で実行）:
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0。
- `corepack pnpm@12.6.0 exec vitest run features/ops` → exit 0、Test Files 6 passed (6)、Tests 49 passed (49)。
- `corepack pnpm@12.6.0 typecheck`（`tsc -b`）→ exit 0。
- `grep -rnE 'celeris/(frontier|standard|cheap)' web --exclude-dir=node_modules --exclude-dir=dist --exclude=schema.json` → 0 件。
- `git diff --check` → exit 0。
- 再実行（コメント変更後）: vitest features/ops → 6 files / 49 tests passed、exit 0。

replan v2 での扱い:
- 前回の `git diff --quiet $(git merge-base HEAD main) -- crates/` は外した（merge-base が main 自身になり検査として誤り）。
- 代わりに `git diff --quiet "$CELERIS_WU_BASE" -- crates/` を流し exit 0（crates 差分ゼロ）。

未解決事項: web 範囲は完了。残りは scripts・docs の leaf と closeout（別 WU）。
