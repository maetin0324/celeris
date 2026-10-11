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
