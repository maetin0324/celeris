---
title: "web のボード・一覧・詳細の木で task の待ちの理由（子・依存・WorkUnit の依存）を出す"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# 待ちの理由（task-wait-reasons）

- 決定: [ADR 2026-10-11-task-wait-reasons](../adr/2026-10-11-task-wait-reasons.md)（Status: 実装済み。D4 の旧 3 欄の削除を除く）
- 共通規則: `/local/celeris/data/workspaces/01M4NAKDCCS5XYAAJ518MVZ134/artifacts/plan-common.md`
- 葉ごとの記録: `2026-10-11-task-wait-reasons/<unit key>.md`
- 完了日: 2026-10-11

## 段と状態

| 段 | 葉 | 状態 |
|---|---|---|
| design | adr | 済（ADR を書いた） |
| design | integrate-design | 済 |
| api | core → api | 済 |
| api | integrate-api | 済 |
| web | web-list ∥ web-tree | 済 |
| web | integrate-web | 済 |
| close | verify | 済（本記録） |

## 葉ごとの要約

- adr: 見送り箇所の棚卸し（D1〜D13・M1〜M9）、`TaskWait` の型、task-core の `db_waits`／`plan_waits`／`unit_waits`、`DaemonSnapshot.task_waits`、API 欄、消す旧欄の表。
- core: `crates/task-core/src/wait.rs` を新設し、`ready_tasks_impl` と dispatcher が同じ判定を呼ぶ。メモリの見送り理由は `DaemonSnapshot.task_waits` に載せる。
- api: task-ops の `wait.rs` で DB → 計画 → 人の gate → メモリの順に待ちを組む。一覧に `waits`・`runnable`・`run_state` の絞り込み、詳細と木に `waits`・段の順・WU の `waiting_on`。旧欄（`pending_children`・`awaiting_children` 系・`TreeNodePhase` の 3 値）は削除。schema と web・gui の生成型を更新。
- web-list: `web/features/tasks/wait-reason.tsx` に理由の表示と辿り先を集約。ボードと一覧に「走れる」印、理由、走れる/待ちの絞り込み。
- web-tree: WU 行に kind と段内の順、未完の依存 key と先行段を表示。子 task 行にも待ちの理由。

## 証拠（2026-10-11、verify 葉）

すべて worktree `repos/agent-platform`、HEAD `d32664ab` 上で実行。

- `bash scripts/dev/test-parallel.sh`（TMPDIR 既定 = 93 文字の run 用 tmp）→ exit 100、`5147 tests run: 5039 passed, 108 failed, 13 skipped`。
  失敗 108 件の要因は `bind: path must be shorter than SUN_LEN`（142 行）と `timed out waiting for fixture readiness`（48 行）。unix socket の path 長の環境要因で、コードの失敗ではない（再掲: 進捗 `core` の記録と同じ型）。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`（試験 fixture だけ短い TMPDIR）→ exit 0、`5147 tests run: 5147 passed, 13 skipped`。summary JSON `passed 5150, failed 0, nextest_exit 0, doctest_exit 0, tmp_leftovers 0`。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo fmt --all -- --check` → exit 0。
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）→ exit 0。
- `corepack pnpm@12.6.0 -C web lint`（`biome check .`）→ exit 0、`Checked 472 files`、`Found 4 warnings`。warning はすべて `web/styles.css:220-223` の `noImportantStyles`。この task の変更ではない（styles.css は今回の task で触っていない）。
- `TMPDIR=/tmp corepack pnpm@12.6.0 -C web exec vitest run` → exit 0、`Test Files 93 passed (93)`、`Tests 679 passed (679)`。
- `TMPDIR=/tmp corepack pnpm@12.6.0 -C web exec node --test server/*.test.mjs` → exit 0、`tests 93, pass 93, fail 0`。
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/wait-reasons.spec.ts e2e/work/task-tree-waits.spec.ts`（TMPDIR=/tmp）→ exit 0、`2 passed`。両方に 390×844 の横溢れ無しの assert を含む。
- 既存の生成型の整合（葉 api で確認済み）: `node web/scripts/gen-types.mjs --check`、`node gui/scripts/gen-types.mjs --check` は api 葉の記録のとおり exit 0。verify ではコードを変えていないので再実行していない。

## 未解決

- ADR D4 の `DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` が削除されていない（`crates/task-ops/src/daemon.rs` に残る。dispatcher の内部集合も同名で残る）。`task_waits` に同じ情報があるので機能は足りるが、ADR の「旧欄は同じ変更で消す」に反する。core・api 葉の範囲外として残し、verify では直さなかった（コードは変えない方針）。
- 一覧の応答で非終端 task の計画を読む負荷は未計測（ADR Consequences）。

## 本番への移行

なし（本番 host・DB・config への操作は無い）。

## 提案

- 後続の小さな task を 1 つ: `DaemonSnapshot` の旧 3 欄と dispatcher の同名集合を `task_waits` へ寄せて消す（ADR D4・D6 の最後の行）。試験は `crates/task-dispatch` の `task_wait_` 系で足りる。
- `gate_waits` で `StageReview`（`human_gate_hold`）を出すかは人が決める（ADR 付記の `api` 葉の未解決）。

## final review 失敗の修正（fix-tests、2026-10-11）

- final review の `bash scripts/dev/test-parallel.sh` は nextest exit 101。原因は `crates/task-ops/src/view/tests.rs` の `task_summary`・`task_detail` の呼び出しが、main 取り込みで入った 4 引数のままだったこと（api 葉の `&crate::wait::MemoryWaits` が無い）。引数を足して直した（挙動は変えていない）。
- 直した後の `cargo nextest run --workspace --no-run` は exit 0。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、5180 passed、13 skipped。`clippy -D warnings` exit 0、`fmt --check` exit 0。
- 既定の TMPDIR では 108 件が socket path 長（146 > 107 バイト）で fixture 待ち timeout により落ちる。これはコードの失敗ではなく run の TMPDIR の長さの問題。詳細は `2026-10-11-task-wait-reasons/fix-tests.md`。
