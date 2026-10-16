---
title: "冪等な試行台帳と Executor trait の拡張（投入前永続化・再開）"
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# ledger（WU ledger）

## 実装済み
- `crates/campaign/src/ledger.rs`: 追記専用 JSONL `out/ledger.jsonl`（本番 SQLite に書かない）。キー `(manifest sha256, problem, arm, trial_no)` → `{state: planned|submitted|waiting|done|cancelled, task_id, run_id, child_task_ids, cluster_job_ids, routing, workspace, submitted_at, reason}`。行ごとに `sync_all`、新規作成時は directory も fsync。キーごとに最後の行が現在値。末尾の不完全行は開く時に切り落とす。`TrialKey::idempotency_key()` が外部に渡す冪等キー。
- `executor.rs`: `Executor` に既定実装つきで `is_async/routing/lookup/submit/resolve/track/poll/cancel` を追加。`execute` と `MockExecutor` は不変。`resolve` が再開手順（同一冪等キーで照会、無ければ投入）。
- `runner.rs`: 非同期実行器では台帳を引いて追跡。投入前に planned を fsync → workspace 作成 → resolve → task_id 追記。Wait は workspace を消さず `waiting` に記録（台帳が参照中の workspace は消さない）。trials.jsonl に記録済みで台帳が未完了なら起動時に done へ補正。予算打ち切り時は未完了の行を `cancel`（task・子 task・クラスタ job の TaskRef 全体、task_id が無い planned は冪等キーで照会して cancel）して `cancelled(budget_stop)` を分母に残す。
- 同期実行器（mock）の Wait は従来どおり記録・残骸なし（`dependency_wait` 試験は不変）。

## mock 検証済み（`tests/ledger_async.rs`、6 件）
- 投入直後の応答喪失 → 再実行で外部 task 3 件・submit 3 回（重複なし）、planned 行が task_id 無しで先に永続化
- planned のまま再起動 / 台帳消失後の再起動（照会で既存 task を拾う）
- Wait 中 workspace 保持と、再開で同じ task を追跡
- 予算打ち切りで未完了 B/C を子 task・job 込みで cancel
- 末尾破損行の切り捨て、冪等キーの安定性

## 実経路検証済み
なし。

## 未実施（後続 unit）
- task 単位 routing 固定欄（task-routing）、A=HarnessExecutor、B/C=CelerisExecutor、CLI `--executor`、ADR 追補 D 番号。
- 二重起動の排他 lock は入れていない（外部側の冪等キーで重複投入は防ぐが、同じ out を同時に 2 つ走らせない運用）。

## 証跡
- `cargo test -p celeris-campaign`: 全通過、`cargo clippy -p celeris-campaign --all-targets -- -D warnings`: 0 件、`cargo fmt --all -- --check`: 差分なし。
