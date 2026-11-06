---
title: 案件の費用対象行と起動前除外数を同じ条件で取得する（usage-query）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# usage-query（Phase 2）

完了日: 2026-11-06

正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D1-2・D1-3、
[2026-11-06-chat-run-launch-boundary-store](../../../../adr/2026-11-06-chat-run-launch-boundary-store.md) S1・S3。新しい設計判断はなく、ADR は足していない。

## 変更（commit は件名で列挙）
- `phase 2: 案件の費用対象行と起動前除外数を同じ条件で取得する`

変更ファイル: `crates/task-core/src/store/project_guard.rs`、同 `project_guard_usage_tests.rs`（新規）、`store/mod.rs`（試験 module の登録）。
- `project_run_usage_between`: chat run は `finished_at` あり・`launch_tracked=1`・`launched_at` NULL・`prelaunch_reason` が 6 値のどれか、の全部を満たす行だけ除く（`cost_state=not_launched` と同じ判定）。task run の規則は変えていない。
- `project_not_launched_runs_between(project_id, since, until) -> u32`: 同じ期間・案件・同じ SQL 述語（`not_launched_sql()`）で除外数を返す。
- `ProjectRunUsage.known_usage`（新欄）: attempt の欠測（`attempts_usage_missing > 0`）がある run は `usage = None`（予算用は不明）にし、読めた usage は `known_usage` に残す。usage を消さず、`task-ops` の既存の集計は変えずに費用不明として数える。

## 修正前の回帰
- コマンド: `cargo test -p task-core --lib project_guard_usage`（試験 `prelaunch_run_is_not_returned_by_usage_sum`、修正前の実装に試験だけを足した状態）
- exit: 101。assertion: `prelaunch run must not be summed: [ProjectRunUsage { model: Some("m1"), usage: None }]`（起動前に閉じた run が和に残り、費用不明として数えられる）。

## 修正後
- 同じコマンド: 4 passed, 0 failed。表形式の試験が固定するもの: 6 reason の除外と件数、旧行（launch_tracked=0）・未知 reason・reason 無し・起動後 usage 無し・usage 破損・attempt 欠測は和に残り unknown、既知 usage は known、進行中は和にも件数にも入らない、別案件と期間境界（`since` を含み `until` を含まない）、除外数と view の `cost_state` の `not_launched` 数の一致、除外数 + 和の行数 = 終わった run 数。
- 試験で見つけた不具合: 述語を `NOT (… AND prelaunch_reason IN (…))` と書くと、reason が NULL の行で式が NULL になり行が落ちる（費用不明の行を黙って消す）。`COALESCE(prelaunch_reason, '')` で防ぎ、reason 無しの行の試験が固定している。

## 未解決・提案
- 既存の view（`chat_run_list` / `run_from_raw`）は、`usage_json` が JSON として正しいが `Usage` として読めない行で一覧全体を `Invalid` エラーにする。cost_state は `unknown` として返す方が D5 に合う。run_store の挙動なので api-contract WU で扱うことを提案する。
- `task_ops::project_budget` はまだ `project_not_launched_runs_between` を呼ばない（`budget_today.not_launched_runs`、fail closed の反映は budget WU・api-contract WU）。
- 基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。
