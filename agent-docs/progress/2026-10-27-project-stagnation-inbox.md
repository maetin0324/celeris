---
title: 案件単位の停滞を決定的に検出して受信箱に出す
tasks: [01M5XXF7QK1WE91TSKPWA3Z47V]
status: done
updated: 2026-10-27
---
# 案件単位の停滞を決定的に検出して受信箱に出す

完了日: 2026-10-27

判断は [ADR 2026-10-27-project-stagnation-inbox](../adr/2026-10-27-project-stagnation-inbox.md)。

## 変更
- `task_ops::stagnation`: 純関数 `detect`（事実・閾値・`now`）、store と releases_dir から事実を読む `gather`、
  `read_unpromoted_releases`、`human_wait_aging`。始まりの時刻は events の `ts` と release の file から毎回求め、
  メモリに状態を持たない。
- 受信箱の種類 `project_stalled`・`human_wait_aging`・`release_unpromoted`・`task_unscheduled`（`InboxKind`）。id は論理キー、
  `created_at` は状態の始まり（revision）。受信箱では答えない（409 `native_action_required`）。
- dispatcher: `check_liveness_and_stagnation`（生存確認の後に停滞を判定。`stalled` の task は除く）、`set_stagnation`、
  `DaemonSnapshot.stagnation`・`stagnation_policy`。
- `TaskStore::latest_event_ts`（task ごとに `(task_id, seq)` の index を 1 回引く）。
- 設定 `[maintenance.stagnation]`（`config/celeris.example.toml` に既定値）。`apply_config` で次の tick から効く。
- API schema・web の生成型、`web/features/inbox/inbox-model.ts`（種類・表示名・native の種類）。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6501 passed / 21 skipped、web Vitest 682 passed、tsc -b 通過）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 停滞の試験: `task-ops` の `stagnation::tests`（6 件: 閾値ちょうどで 1 件、再検出で同じ key・since、解消で消える、
  release の読み、gather）、`human_inbox::tests` の `human_wait_aging_is_one_item_per_project_and_closes_when_answered`・
  `stagnation_findings_map_to_items_keyed_by_logical_key`、`task-dispatch` の
  `release_unpromoted_appears_at_threshold_once_and_closes_on_promotion`（偽の時計）。

## commit（件名）
- 案件単位の停滞を決定的に検出して受信箱に出す（project_stalled・human_wait_aging・release_unpromoted・task_unscheduled）
- 停滞の判定の状態を StagnationWatch にまとめ、dispatcher.rs の行数を増やさない
- 進捗: 案件単位の停滞の検出

## 本番への移行
- 設定の書き換えは不要（`[maintenance.stagnation]` は省略時に既定値）。閾値を変えるなら本番 config に節を足す。

## 未解決事項
- `human_wait_aging` は snapshot が無い呼び出し（`celeris::notify::triage` の `source_unresolved` 等）では出ない。
  既存の `stalled` と同じ扱い。
- web の受信箱画面の e2e は走らせていない（release gate で走る）。

## 提案
- CoS triage の `source_unresolved` に snapshot を渡せば、停滞の項目も送信直前の照合で生きていると判定できる。
