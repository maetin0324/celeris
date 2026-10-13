---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# detect-fix

## 原因

段 detect の統合で `task-api schema::tests::committed_schema_matches_generated` が落ちた。sweep-core が `task-ops` の `InboxKind` に `Unintegrated` を足したが、`docs/api/v1/api-v1.schema.json` を再生成していなかった（schema drift）。合わせて web の `InboxKind` 網羅（`KIND_LABELS: Record<InboxKind, string>`）も、新しい kind を知らないと型検査が落ちる状態だった。

## 行った変更（挙動の追加なし）

- `docs/api/v1/api-v1.schema.json` を `UPDATE_SCHEMA=1 cargo test -p task-api schema` で再生成。差分は `InboxItem` に `unintegrated` 項目（`task`・`repo_id`・`branch`・`head_sha`・`reason`・`at`）と `InboxKind` に `"unintegrated"` の 2 点だけ（41 行追加・1 行変更）。
- `gui/app/celeris/types.ts` を `corepack pnpm@11.27.0 -C gui gen:types` で再生成（12 行）。
- `web/api/generated/schema.json` と `web/api/generated/types.ts` を `corepack pnpm@12.6.0 -C web gen:types` で再生成（42 行・10 行）。
- `web/features/inbox/inbox-model.ts`: `INBOX_KINDS` に `"unintegrated"` を足し、`KIND_LABELS` に `unintegrated: "未取り込み"` を足した（網羅のための最小変更）。`NATIVE_KINDS` には足さない。この item は `merge`・`discard`・`hold` の選択肢を持つので、通常の答え経路に乗る。
- `docs/protocol/` と `web/api/realtime/` は変更なし（event の schema に差分が無く、realtime の網羅は型検査で通った）。

## 実行したコマンドと結果

- `UPDATE_SCHEMA=1 cargo test -p task-api schema` — 再生成を実施。
- `cargo test -p task-api --no-fail-fast` — exit 101。`committed_schema_matches_generated` と `schema_endpoint_returns_the_committed_file` は `ok`。失敗は 8 件のみで、すべて browser 系 (`browser_e2e` 4 件、`browser_live_cross` 3 件、`browser_waits` 1 件。credentiald の socket readiness 待ち、live view の CDP 経路)。他の test binary は全て `ok`（`test result: ok` が 77 件）。
- `corepack pnpm@11.27.0 install --offline` / `corepack pnpm@12.6.0 install --offline` — exit 0（各 dir）。
- `corepack pnpm@11.27.0 -C gui gen:types` / `corepack pnpm@12.6.0 -C web gen:types` — exit 0。
- `corepack pnpm@11.27.0 -C gui typecheck`（react-router typegen + tsc -b）— exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`（tsc -b）— exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/inbox` — 5 passed。
- `biome check features/inbox/inbox-model.ts` — No fixes applied（clean）。
- `bash scripts/dev/test-parallel.sh` — exit 100。`5133 tests run: 5025 passed, 108 failed, 13 skipped`。**失敗は 108 件、すべて既知の sandbox 失敗の族**（browser・launcher・credentiald（`celeris-credentiald::injection_ipc` 42 件）・CDP・live view）。内訳の binary: `task-worker`（browser_launcher / browser_control_gate_wire / browser_live_cross / browser_shared_cdp）、`celeris-credentiald::injection_ipc`、`task-api`（browser_e2e・browser_live_cross）、`celeris`、`celerisctl::approve_talks_one_line_to_a_private_socket`。stranded・inbox・schema・store・migration に当たる失敗は 0 件。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished` 1m 01s）。

## 既知の失敗（この変更と無関係と判断した根拠）

- 失敗はすべて browser・credentiald・launcher・CDP・live view の試験。task 取り込み・受信箱・schema の試験は通る。
- `git diff b88f61b9 HEAD -- crates/celeris-credentiald crates/task-api/tests/browser_e2e.rs` は空（この branch は credentiald と browser 試験に触れていない）。
- 失敗の形は、子 process が socket を作れず待ちが時間切れになる（`timed out waiting for fixture readiness`）。`agent-platform` の既知の sandbox 失敗（credentiald・userns・CDP）と同じ族。
- 本 WU では基点（base）で同じ test を流して比べてはいない（別 tree の full rebuild が要るため）。sandbox の外で `test-parallel` を流せば確定できる。確定の手順は「未解決事項」を見る。

## 未解決事項

- 上の 108 件が sandbox 外の本物の失敗か、sandbox だけの失敗かは未確定。sandbox 外（ユーザ namespace と `/run/user` が使える環境）で `bash scripts/dev/test-parallel.sh` を流せば確定する。
- 受け入れ条件 0「`cargo test -p task-api` が exit 0」は満たしていない（exit 101、上記 8 件は browser 系）。schema drift は解消済み。

## 提案

- sandbox の失敗を gate 上で区別できるように、browser 系の試験に既知の sandbox 印（環境変数など）を付け、その試験を sandbox 外でだけ要求する形にする。

## 再検証（2026-10-11, 2 回目の run）

- `UPDATE_SCHEMA=1 cargo test -p task-api schema` の後も `git status --short` は空。生成物（docs/api/v1・gui・web の生成型）は commit 済みの d4266df9 と一致し、差分は出ない。
- `cargo test -p task-api schema` — `schema::tests::committed_schema_matches_generated ... ok`、`schema_endpoint_returns_the_committed_file ... ok`。
- `cargo test -p task-api --no-fail-fast` — 失敗は browser_e2e・browser_live_cross・browser_waits の browser 系だけ（前回と同じ族）。受け入れ条件 0 の「exit 0」はこの browser 系が sandbox で落ちるため未達。
- 前回の run の check 不合格（`out of scope: web/features/inbox/inbox-model.ts`）の原因: `KIND_LABELS: Record<InboxKind, string>` の網羅のため、このファイルの変更は型検査に必要。範囲 check の許可 path（crates/・docs/api/v1/・docs/protocol/・gui/app/・web/api/・進捗ディレクトリ）に `web/features/inbox/` が無い。ファイルを戻すと web の typecheck が落ちるため、範囲 check 側の修正（plan_issue）を申告する。
- test-parallel の結果は前回 run（同じ commit d4266df9）の記録のとおり。この run では再実行していない。
- 2 回目の run で `web/features/inbox/inbox-model.ts` を基点の版へ戻して `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）を流したところ exit 1（`features/inbox/inbox-model.ts(25,14): error TS2741: Property 'unintegrated' is missing ... Record<InboxKind, string>`）。`tsc --noEmit -p .` は戻した状態でも通るが、それは `tsc -b` と同じ検査ではないため誤判定だった。よってこのファイルの変更は必要で、d4266df9 の版（`unintegrated` 追加）を残した。
- 範囲 check が落ちる原因は、check の許可 path に `web/features/inbox/` が無いこと。成果を直しても（戻せば typecheck が落ち、残せば check が落ちる）check が通らないため plan_issue を申告する。

## 提案（範囲 check の修正）

- 検出 unit（detect-fix）の範囲 check の許可 path に `web/features/inbox/` を足す。`InboxKind` に variant を足す葉は web の `KIND_LABELS: Record<InboxKind, string>` と `INBOX_KINDS` も更新が要る（`web/api/generated/` だけでは型検査が落ちる）。

## 再検証（2026-10-11, 3 回目の run）

- 対象 commit: d4266df9（inbox-model.ts の `unintegrated` 追加を含む。この run で追加の変更は無し）。
- `cargo test -p task-api schema` — `committed_schema_matches_generated ... ok`、`schema_endpoint_returns_the_committed_file ... ok`、exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）— exit 0。`inbox-model.ts` の `unintegrated` が無いと TS2741 で落ちるため、このファイルの変更は必要（前 run で確認）。
- `corepack pnpm@12.6.0 -C web exec vitest run features/inbox` — 5 passed。
- `bash scripts/dev/test-parallel.sh` — exit 100。`5133 tests run: 5025 passed, 108 failed, 13 skipped`（同じ commit の 2 回目の run と同数）。失敗は sandbox 族のみ: `celeris-credentiald::injection_ipc` 17 件（ほか broker 3・browser_doctor 1）、`task-api::browser_e2e` 3 件・`browser_live_cross` 3 件、`celeris` 2 件、`celerisctl::bin/celerisctl`（approve の private socket）1 件。stranded・inbox・schema・store の試験は失敗 0 件。
- 受け入れ条件 0「cargo test -p task-api が exit 0」は未達（browser 系の sandbox 失敗のため）。
- 範囲 check（`scope`）は依然として `web/features/inbox/inbox-model.ts` で落ちる。Objective は web/features/ を範囲内としているが、check の許可 path（crates/・docs/api/v1/・docs/protocol/・gui/app/・web/api/・進捗ディレクトリ）には無い。check 側の修正が要る（plan_issue）。

## 再検証（2026-10-11, 4 回目の run）

- 人の回答（承認）により、範囲 check の許可 path に `web/features/inbox/` を含めた。`web/features/inbox/inbox-model.ts` の変更（`unintegrated` の INBOX_KINDS と KIND_LABELS）は残した。範囲 check の再実行 — `scope ok`（変更 path は docs/api/v1/・gui/app/・web/api/・web/features/inbox/・進捗ディレクトリのみ）。
- `cargo test -p task-api schema` — `committed_schema_matches_generated ... ok`、exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）— exit 0。
- `bash scripts/dev/test-parallel.sh` — exit 100。`5133 tests run: 5025 passed, 108 failed, 13 skipped`（前 run と同数）。失敗は sandbox 族のみ: `celeris-credentiald::injection_ipc` 17、`broker` 3、`browser_doctor` 1、`task-api::browser_e2e` 3、`browser_live_cross` 3、`celeris` 2、`celerisctl::bin/celerisctl` 1。stranded・inbox・schema・store の試験は失敗 0。
- 受け入れ条件 0「`cargo test -p task-api` が exit 0」は未達（browser_e2e・browser_live_cross の 6 件が fixture 待ちで落ちる。sandbox 族）。この run では sandbox 外での再確認はしていない。
