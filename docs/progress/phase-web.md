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

## Phase 3（完了 2026-09-30、中核の画面 P3-01〜P3-15）

P3-01〜P3-15 は各 1 commit 以上（web phase 3 の全 commit は `git log --oneline --grep 'web phase 3'` で拾える）: P3-01 `bd00a32c`（Console 中継と cache）、P3-02 `cd421e3d`（Console の画面）・`7891d3a7`（parity 完了）・`fe7f6f6b`（screens.test.ts の型直し、replan v2 の r2 葉）、P3-03 `6a0bbbc7`（inbox と共通の操作フィードバック）、P3-04 `6e9eb5d4`（approvals と常設ルール）、P3-05 `753e272f`（タスク一覧の絞り込み・並び）、P3-06 `f9e74443`（タスクと計画の作成画面）、P3-07 `6937011f`（依存グラフ）、P3-08 `23ebdd61`（タスク詳細の枠・overview・timeline）、P3-09 `744f660a`（判断パネル）、P3-10 `0a2bfd68`（実行・routing 操作）・`b96485d4`（e2e の port 7720 衝突回避）、P3-11 `127ddb46`（作業ツリー・成果物 viewer、H8）、P3-12 `8d35dbee`（run ログの会話表示）、P3-13 `cfe6b400`（変更 tab と 5 tab の結合）、P3-14 `cafcaf76`（報告の一覧・詳細）、P3-15 `44cf8508`（ブラウザ通知、タブ間 1 回、H9）。parity の補修は `ccb95796`（R21・R35 close）、`d2b142dd`（task・graph の絞り込みと history の同期）、`f4a22a16`+`8b6ddc97`（R28 を `POST /tasks` の root task 作成へ移す）。

Phase 3 の完了検証は turn 切れを避けるため 3 葉（p3-close-static → p3-close-e2e → p3-close-record）に分割した（前試行の p3-close は同じ検証を 4 回繰り返して turn 切れを繰り返した）。

- p3-close-static（前 p3-close の branch を `--ff-only` 取り込み、V1・V2・parity 台帳）: `git merge-base --is-ancestor f4a22a16 HEAD && git merge-base --is-ancestor 8b6ddc97 HEAD` → exit 0（両方 ancestor）。`corepack pnpm@11.27.0 -C gui test && typecheck && build` → exit 0。`corepack pnpm@12.6.0 -C web install --frozen-lockfile && typecheck && lint && test && build`、続けて `gen:types --check && check:boundaries && check:secrets` → 各 exit 0。`node web/scripts/check-parity.mjs --require-phase 3` → exit 0（Phase 3 対象の parity 行 R01・R02・R08・R17〜R28・R35・R38・R39・X9・X14 はすべて「完了（\<commit\>）」）。修正 commit は発生せず（作業ツリーは clean のまま）。
- p3-close-e2e（parity e2e 全件と V3 の e2e）: p3-close-static の branch は既に HEAD の祖先。`corepack pnpm@12.6.0 -C web e2e parity/ latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts` → 89 件中 86 passed・3 skipped（fixture screenshots のみ）、exit 0。typecheck（`tsc -b`）・lint（`biome check`）も exit 0。落ちた spec が無く web/ の修正は不要だった（作業ツリーは clean のまま）。
- p3-close-record（この葉、cargo と記録）: `git merge --ff-only celeris-wu/.../p3-close-e2e` → 既に取り込み済み（up to date）。`cargo test --workspace` → exit 0（テストバイナリ 95 個すべて `test result: ok`、合計 2886 passed・0 failed）。`cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。

V1〜V3 と parity の受け入れ条件（このタスク全体の受け入れ条件 0〜2）は上記 3 葉の検証をあわせて満たす。各画面の V3（遷移の独立・再取得の範囲・mobile 4 幅・axe）は個々の P3-NN 葉と p3-close-e2e の a11y/axe・latency/transition・realtime/refetch-scope spec で確認済み。

- 未解決: Phase 3 の parity 台帳で Phase 4/5 に属する行（R29〜R34・R36、X10 の mobile-audit 全画面）は引き続き「未着手」（計画どおり Phase 3 の範囲外）。
- 提案: Phase 4 では `check-parity.mjs --require-phase 4` を新しい gate にし、R29〜R36 と X10（全画面の mobile-audit・axe）をまとめて閉じる。close 葉を 3 分割する運用（静的検査 / e2e / cargo+記録）は turn 切れを避けられたため、以後の Phase close でも踏襲する。

## Phase 4（完了 2026-10-01、管理の画面 P4-01〜P4-17）

P4-01 `35be337a`、P4-02 `17c7df81`、P4-03 `6959502d`、P4-04 `1467aaf6`、P4-05 `d7c172f0`・`70eac060`、P4-06 `a09d4984`、P4-07 `f2b1a39c`、P4-08 `9487d16c`、P4-09 `f6d1a44a`・`990feeb9`、P4-10 `0ca70336`、P4-11 `87d719dc`、P4-12 `22eb1d21`・`7209dd11`、P4-13 `cce734b8`・`108246b2`、P4-14 `352bd32a`、P4-15 `362b244b`、P4-16 `068ffdf9`、P4-17 `8b63df32`。追加の parity 記録 commit は `ea26b63a`（組織・ヘルプ）と `0bc1b21e`（R11〜R13）。P4-01/P4-02 の実装と一部 parity 完了記録は同じ Phase 4 commit 群に含む。

Phase 4 の close は `p4-close-static` → `p4-close-e2e` → `p4-close-record` の 3 葉。

- p4-close-static: `corepack pnpm@11.27.0 -C gui install --frozen-lockfile`、GUI の typecheck/test/build → exit 0（81 files / 1222 tests）。`corepack pnpm@12.6.0 -C web install --frozen-lockfile`、web の typecheck/lint/test/build と `gen:types --check`・`check:boundaries`・`check:secrets` → 各 exit 0（Vitest 24 files / 174 tests、node:test 41 件）。`node web/scripts/check-parity.mjs --require-phase 4` → exit 0。R06・R07・R09〜R16・R29〜R34・R36 は `完了（<commit>）`。
- p4-close-e2e: `corepack pnpm@12.6.0 -C web e2e parity/ latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts` → exit 0、157 passed / 5 skipped（screenshots）。typecheck・lint → exit 0。Phase 4 の16画面の `mobile-audit --only <path>` → 全て exit 0。
- p4-close-record（2026-10-01、検査時 HEAD `3afa3767404cc237a05b0b80653f884b4034c435`）: Celeris の cargo 環境をそのまま使い、sandbox 外で `cargo test --workspace` → exit 0（2,886 passed / 0 failed / 7 ignored、warning 0）、`cargo clippy --workspace -- -D warnings` → exit 0（warning 0）。ログ: run artifacts の `cargo-test.log`・`cargo-clippy.log`。前回は sccache の EPERM で一度失敗し、2026-10-01 に再実行して exit 0。Rust ソースはこの Phase 4 web 作業で変更していない。

Parity で閉じた Phase 4 行は R06・R07・R09〜R16・R29〜R34・R36（`docs/web/feature-parity.md` の完了 commit 参照）。R08 は Phase 3 の Console として完了済み。V3 e2e と mobile-audit は上記のとおり成功。X10 全画面 axe gate は Phase 5 の横断 gate として未解決。cargo test/clippy は 2026-10-01 の再実行で exit 0。Phase 4 cargo gate は完了。

- 未解決: X10 全画面 axe gate は Phase 5 に残る。crates の flaky 修正（dispatcher.rs・ssh.rs）はこの web 差分から戻してあり、main への別途投入が必要。
- 提案: Phase 5 で X10 の全画面 axe/mobile gate を閉じ、crates の flaky 修正を main に別途投入する。
- prune テストの event 待ちと `ssh.rs` stub の ETXTBSY 修正は web の差分外なので戻した。main（分割後の dispatcher）へ別途入れる。

## Phase 5（完了 2026-10-01、横断 gate P5-01〜P5-04）

P5-01〜P5-03 の測定記録は [latency gate](../web/gates/p5-01-latency.md)、[security gate](../web/gates/p5-02-security.md)、[mobile・a11y gate](../web/gates/p5-03-mobile-a11y.md)、P5-04 の総点検は [feature parity matrix](../web/feature-parity.md) に記録した。X10/X11/X15 は完了。

- P5-01 latency: `corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts realtime/refetch-scope.spec.ts parity/latency-gate.spec.ts` → exit 0。偽 daemon/gateway、JSON 0/5/10 秒遅延で全 30 path を計測。最大値は URL 69.4 ms、見出し 90.1 ms、10 秒−0 秒の最大差は URL 15.5 ms・見出し 16.5 ms。閾値は URL/見出し各 300 ms 以下、差各 100 ms 以下で全件合格。画面固有の無関係イベント再取得は全画面 0 本。H1 project fallback は fixture の 4 event 中 1 回（fixture 実測で本番頻度を示さない）。
- P5-02 security: `corepack pnpm@12.6.0 -C web typecheck` と `corepack pnpm@12.6.0 -C web e2e parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts parity/shell.spec.ts` → exit 0、20/20。最終変更後の横断 security/storage 2 spec も exit 0、2/2。X1〜X6/X8 を確認し、許可外 Host は全経路 400、CSRF は 403、保護経路の未認証・不正 cookie は 401。security header と token 非露出が合格。全 31 台帳行を開いた直後に localStorage/sessionStorage/IndexedDB/Cache Storage/Service Worker を検査し、表示設定以外の秘密・API 本文の永続化なし。
- P5-03 mobile/a11y: `corepack pnpm@12.6.0 -C web mobile-audit` → exit 0（30 path × 360/390/412/1440、各幅の横溢れ 0、タップ/名前/構造/focus 判定 ok）。`corepack pnpm@12.6.0 -C web e2e a11y/ parity/mobile-gate.spec.ts --workers 4` → exit 0、191 passed。全 path/幅で axe critical/serious 0。検出された `/providers` checkbox などのタップ領域は修正済み。
- P5-04 総点検: `docs/web/feature-parity.md` の X10・X11・X15 を gate 証拠と照合し、完了（X10 `ba629384`、X11 `46b3275f`、X15 `4d41b3e3`）を確認。X11 の閾値は遷移 URL/見出し各 300 ms 以下、10 秒遅延と 0 秒の差各 100 ms 以下、無関係イベントによる画面固有再取得 0 本。
- 未解決: gate は loopback 偽 daemon/gateway と fixture による検証で、本番頻度や実環境の遅延分布は測っていない。dogfood 開始条件 H6 は人の決定待ち。X15 の staging 実機確認と gui/web 配信切替も運用段階に残る。
- 提案: H6 と H9 の扱いを決めてから dogfood を開始する。開始前に H10 の staging 確認手順を実施し、配信切替は H7 の判断材料を確認して決める。

## Phase 6（P6-01〜P6-03 完了 2026-10-01、並行運用の準備。P6-04 以降は未着手）

P6-01 は `pnpm -C web release` が web の配布物を生成すること、P6-02 は ADR-0096・`celeris-web@.service`・selfdeploy の非 blocking web 段と `tests/release_web_stage_nonblocking.sh`、P6-03 は [dogfood 手順](../web/dogfood.md) を整備した。dogfood は H6 の決定まで未開始であり、本番 daemon への接続はしていない。

- Rust gate: `cargo test --workspace` → exit 101。全 7 test suites で 261 passed、2 failed、0 ignored。`crates/celeris/tests/releases_api.rs` は 8 件中 6 passed・2 failed。失敗した `promoting_a_verified_release_starts_the_bundled_script_and_returns_202` と `promoting_prefers_the_promote_script_of_the_current_release` は、user scope bus への接続エラー（`Failed to connect to user scope bus via local transport: No data available`）。`cargo test -p celeris --test releases_api` でも同じ 2 件を再現（6 passed / 2 failed）。2026-10-01 の人の判断に従い、この sandbox から user systemd bus に接続できない環境由来の 2 件として除外し、残りの全テストを合格として扱う。テスト側の skip は別 task で対応する。
- Rust lint: `cargo clippy --workspace -- -D warnings` → exit 0（warning 0）。
- 未解決: H6（dogfood の期間・合格条件）、H9（並行運用中の通知）、H10（staging 実 celeris 確認）は未決／未確認。H7（gui/web 配信切替）の判断も未実施。P6-04 以降は未着手。
- 提案: H6 と H9 を決め、H10 staging 確認を記録してから dogfood を開始する。配信切替は H7 の判断材料を確認したうえで別途判断する。P5 横断 gate の値（30 path の URL/見出し最大 69.4/90.1 ms、H1 fallback 1 回、30 path × 4 幅の mobile/a11y 合格）は Phase 5 節と各 gate 記録を参照。

### P2-07 V3 台帳のレビュー修正（2026-09-30）

修正 commit `web phase 2 P2-07: select V3 screens from the ledger`（本節を含む commit）。`screens.ts` の `v3: true` が付いた画面だけを S1・S2・S4 が選ぶようにし、Phase 2 では `/tasks` と `/inbox` のみを対象にした。nav に無い画面は台帳の fixture で開く。動的 route も見出しを起点にデータ領域を確認する。

- 再走: `web/node_modules/.bin/tsc -b`、`web/node_modules/.bin/biome check .`、`web/node_modules/.bin/vitest run`、`node web/scripts/check-parity.mjs --require-phase 2`（前 3 件は `web/` で実行）→ 各 exit 0、vitest 16 files / 148 tests（台帳テスト 2 件を含む）pass。
- 再走: `web/node_modules/.bin/playwright test latency/transition.spec.ts realtime/refetch-scope.spec.ts a11y/axe.spec.ts parity/`（`web/` で実行）→ exit 0、27 passed（S1・S2・S4 各 2 件、parity 21 件）。S1 の selector 修正後に V3 の 6 件を再走し、6 passed。偽 daemon と gateway は loopback の空き port を使用。
- 拡張確認: `/tasks/$id` に一時的に `v3: true` を付け、`-g '/tasks/\$id'` で S1・S2・S4 の 3 件 pass。印は確認後に戻した。`corepack pnpm@12.6.0 -C web` はこの worktree の依存未配置から外部取得を試みたため停止し、同じ固定版のローカル依存をコピーして上記の binary を直接実行した。
