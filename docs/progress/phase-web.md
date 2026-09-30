# PROGRESS — Web GUI（ADR-0081、`web/`）

計画の正本: [implementation plan](../web/implementation-plan.md)、[feature parity matrix](../web/feature-parity.md)、[ADR-0081](../adr/0081-web-spa-frontend.md)。Phase 0 の記録は [PROGRESS.md](../PROGRESS.md#web-gui-phase-02026-09-29設計移行計画) に残す。

## Phase 1（完了 2026-09-30、scaffold と gateway）

P1-01〜P1-09 は各 1 commit: P1-01 `7cb24031`、P1-02 `8ab4362c`、P1-03 `809cd581`、P1-04 `96717f83`、P1-05 `d8d375cd`、P1-06 `8eae18e0`、P1-07 `77d10ac9`、P1-08 `3bc52d54`、P1-09 `f0835f61`。parity の X16・R03・X2〜X6・R05・R04・X1・R40・R41 を完了、R37 は実装中（中継まで。再接続と invalidate は Phase 2）。

P1-01〜P1-09 の実装を完了。React SPA の scaffold、型生成・偽 daemon、Express gateway、独立 session と login、JSON・file・SSE 中継を追加した。parity の R40・R41・X6 を完了、R37 は中継まで実装済みで、再接続と invalidate は Phase 2 に残る。auth の改竄 mac テスト（末尾が偶然 'A' だと改竄にならない 1/64 の flaky）を固定文字置換から「元と必ず異なる 1 文字」に直した（`web/server/auth.test.mjs`）。

- 証拠: `node --test web/server/auth.test.mjs` を 20 回連続 → 全 exit 0（flaky 修正の確認）。
- 証拠: `git diff --quiet ecbd5be19f76 -- . ':!web' ':!docs/web' ':!docs/PROGRESS.md' ':!docs/progress'` → exit 0。
- 証拠: `corepack pnpm@11.27.0 -C gui test && corepack pnpm@11.27.0 -C gui typecheck && corepack pnpm@11.27.0 -C gui build` → exit 0（vitest 81 ファイル 1222 件 pass、typecheck・build とも成功）。host の pnpm は 12.6.0 だが `corepack pnpm@11.27.0` で `gui/package.json` の固定版のまま実行でき、版合わせの別 task は不要だった。
- 証拠: `pnpm -C web install --frozen-lockfile && pnpm -C web typecheck && pnpm -C web lint && pnpm -C web test && pnpm -C web build` → exit 0（vitest 5 ファイル 14 件、`node --test server/*.test.mjs` 36 件、すべて pass）。
- 証拠: `pnpm -C web gen:types --check && pnpm -C web check:boundaries && pnpm -C web check:secrets && pnpm -C web check:parity` → exit 0。
- 証拠: `pnpm -C web e2e parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts` → 14 件 pass。偽 daemon と gateway は loopback の空き port を使用し、SSE が 60 秒を超えて流れ続け、切断で upstream が abort されることを確認。
- 証拠: `cargo test --workspace && cargo clippy --workspace -- -D warnings` → exit 0。
- 未解決: R37 の SSE 再接続・invalidate 表は中継のみで Phase 2 に残る。
- 提案: Phase 2 で R37 の再接続・invalidate 実装と、V3（画面の共通検査）を揃える。

### 再確認（2026-09-30、task 01M3RPJNN0YESBPDZ50M7H2A3Y、前試行のブランチを merge した `03d217e1` で、pnpm は corepack で版を固定）

- 証拠: `git diff --quiet $(git merge-base HEAD main) -- gui crates docs/api deploy scripts/selfdeploy && corepack pnpm@11.27.0 -C gui install --frozen-lockfile --prefer-offline && corepack pnpm@11.27.0 -C gui test && corepack pnpm@11.27.0 -C gui typecheck && corepack pnpm@11.27.0 -C gui build` → exit 0（81 files / 1222 tests pass）。
- 証拠: `corepack pnpm@12.6.0 -C web` の install --frozen-lockfile・typecheck・lint・test・build・gen:types --check・check:boundaries・check:secrets・check:parity → 各 exit 0。
- 証拠: `corepack pnpm@12.6.0 -C web e2e parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts` → exit 0、14 passed。
- 証拠: `cargo test --workspace` → exit 0（2886 passed、0 failed）、`cargo clippy --workspace -- -D warnings` → exit 0。

## Phase 2（完了 2026-09-30、shell と realtime）

P2-01 `2194da2f`、P2-02 `8bc88050`、P2-03 `bcfe6276`、P2-04 `279d2bef`、P2-05 `c353ac0b`、P2-06 `89416a54`、P2-07 `156f2929`（各 ID の実装 commit）。R37・R42・X7・X8・X12・X13 の parity 行は対応する e2e が通過した commit で完了としている。P2-07 は画面台帳と V3 の枠を追加し、`/tasks`・`/inbox` の shell 画面で S1〜S4 を確認した。

- V1: `git diff --quiet $(git merge-base HEAD main) -- gui` → exit 0。Phase 2 の設計判断は [ADR-0082](../adr/0082-web-sse-invalidate-unlisted-kinds.md) に記録した。`gui/node_modules/.bin/vitest run && gui/node_modules/.bin/tsc --noEmit && gui/node_modules/.bin/vite build`（`gui/` で実行）→ exit 0（81 files / 1222 tests、typecheck、build）。
- V2: `web/node_modules/.bin/tsc -b`、`web/node_modules/.bin/biome check .`、`web/node_modules/.bin/vitest run && node --test web/server/*.test.mjs`、`web/node_modules/.bin/vite build` → 各 exit 0（vitest 15 files / 146 tests、node 38 tests）。`node web/scripts/gen-types.mjs --check`、`check-boundaries.mjs`、`check-secrets.mjs`、`check-parity.mjs --require-phase 2` → 各 exit 0。
- parity: `web/node_modules/.bin/playwright test parity/`（web/ で実行）→ exit 0、21 passed。偽 daemon と gateway は loopback の空き port を使用。
- V3: `web/node_modules/.bin/playwright test latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts`（`web/` で実行）→ S1・S2 各 2 pass、S4 2 pass。S1 の URL / 見出しは全条件 300 ms 以下で、10 s と 0 s の差は 100 ms 以下。S2 は active な inbox query を監視し、2 s ごとの daemon tick 10 件と無関係な worker progress 20 件で、15 s の補完取得を超える再取得がないことを確認。`node web/scripts/mobile-audit.mjs --only /tasks` と `--only /inbox` → 各 exit 0、4 幅。`node web/scripts/screenshots.mjs --only /tasks --out <run artifacts>/shots` → exit 0、4 枚。画面台帳から route を削る単体テストは `check:parity` の失敗を確認。
- Rust: `cargo test --workspace`、`cargo clippy --workspace -- -D warnings` → 各 exit 0。
- 再確認（同日、固定 pnpm）: `corepack pnpm@12.6.0 -C web install --frozen-lockfile`（axe-core 込み）・typecheck・lint・test・build・check:parity → 各 exit 0。`corepack pnpm@12.6.0 -C web e2e parity/ latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts` → exit 0、27 passed。`CI=true corepack pnpm@11.27.0 -C gui install --frozen-lockfile`・test・typecheck・build → 各 exit 0（81 files / 1222 tests）。`cargo test --workspace` → exit 0（2886 passed）、clippy → exit 0。
- 修正: P2-03 の shell は 401 で `/login?next=` へ移るため、`parity/gateway-auth.spec.ts` の cookie 消去直後の `page.goto` がその遷移と競って落ちた（27 件中 1 件）。cookie を消す前に `about:blank` へ移すようにし、`--repeat-each 5` を 3 回（60/60）通した。
- 未解決: なし（V3 の S3 mobile-audit・screenshots は /tasks・/inbox のみ。他画面は Phase 3 で台帳に沿って足す）。
- 提案: Phase 3 の各画面で V3 の台帳と検査を使い、P5-01 で全画面の遅延 gate と継続的な SSE tick を実測する。

### P2-07 V3 台帳のレビュー修正（2026-09-30）

修正 commit `web phase 2 P2-07: select V3 screens from the ledger`（本節を含む commit）。`screens.ts` の `v3: true` が付いた画面だけを S1・S2・S4 が選ぶようにし、Phase 2 では `/tasks` と `/inbox` のみを対象にした。nav に無い画面は台帳の fixture で開く。動的 route も見出しを起点にデータ領域を確認する。

- 再走: `web/node_modules/.bin/tsc -b`、`web/node_modules/.bin/biome check .`、`web/node_modules/.bin/vitest run`、`node web/scripts/check-parity.mjs --require-phase 2`（前 3 件は `web/` で実行）→ 各 exit 0、vitest 16 files / 148 tests（台帳テスト 2 件を含む）pass。
- 再走: `web/node_modules/.bin/playwright test latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts parity/`（`web/` で実行）→ exit 0、27 passed（S1・S2・S4 各 2 件、parity 21 件）。S1 の selector 修正後に V3 の 6 件を再走し、6 passed。偽 daemon と gateway は loopback の空き port を使用。
- 拡張確認: `/tasks/$id` に一時的に `v3: true` を付け、`-g '/tasks/\$id'` で S1・S2・S4 の 3 件 pass。印は確認後に戻した。`corepack pnpm@12.6.0 -C web` はこの worktree の依存未配置から外部取得を試みたため停止し、同じ固定版のローカル依存をコピーして上記の binary を直接実行した。
