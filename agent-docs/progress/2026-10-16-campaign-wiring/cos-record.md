---
unit: cos-record
task: 01M530Y2MXJFVQBWJSP21F02DE
status: done
tests: "test-parallel 5523 passed 0 failed; clippy exit 0; fmt exit 0"
completed: 2026-10-16
---
# cos-record: CoS 実 turn・人の調整・復旧介入の記録と通し試験

## 着手時の確認
- 型 `DecisionLogUsage::cos_turn` と `decision_log_interventions` は main にあったが、本番コードに書き手が無かった（試験だけが使用）。
- 既に直っている項目（api-ident / executor / propagate / jev-calls）は各 progress の commit と試験を参照し、再実装していない。identity 側の未適用 patch は api-ident で取り込み済み（propagate.md）。

## 実装済み
- `SqliteStore::decision_log_project_cos_turns`: 終端 `chat_runs` を CoS turn の共有オーバーヘッド消費行へ（冪等、欠測は 0 にしない、費用は定価換算欄のみ）。帰属は活動窓に 1 campaign のときだけ。
- `SqliteStore::decision_log_project_interventions`: events から人の介入（Answered / Retried / ApprovalDecided / 人の Edited）を `<task>:<seq>` で冪等に投影。
- `task_dispatch::record_task_decision_log`（pub）: dispatcher の `record_decision_log` と通し試験が同じ経路。TaskStore trait に 2 method 追加。
- 再投影で `proposed_first`/`proposed_candidates` が消えないよう保護。ADR: `agent-docs/adr/2026-10-16-cos-turn-intervention-recording.md`。

## mock検証済み
- `crates/celeris/tests/campaign_wiring_e2e.rs`: 実 HTTP の `POST /tasks`（temp DB・loopback）→ 再送 200・別 arm は別 task → 子・孫（POST の `parent`）→ planner / 失敗再試行 / reviewer の run events → CoS の実 turn → Jev 3 呼び出し（失敗・成功・再分類、費用欠測は NULL）→ 人の回答 → 3 回再投影しても行・消費・介入が増えない → `decision_log_export_campaign` + `verify_connected` が問題なし。識別の直挿入なし。窓外の CoS turn は帰属なし。
- 範囲外: Jev の 3 呼び出しは `decision_log_record_usage` で書いた（実 `record_jev_usage` 経路は jev-calls の dispatcher 試験が検証）。routing 提案・grader 判定は各 writer の値を試験が書いた。executor の `body` を実 server へ送る接続は campaign crate を celeris の dev-dep にしないため未接続（body の形は executor 試験、受け口は campaign_ident が検証）。

## 実経路検証済み
- なし（本番 SQLite/KB・実 LLM は触っていない）。

## 承認待ち
- なし。本測定の開始・予算増額はしていない（3M / case 30万 / arm 130万 / 並列 1 のまま）。

## 提案
- 手動の復旧・merge は event に出ないので、評価者が最小入力ログで `decision_log_record_intervention` を呼ぶ CLI（`celerisctl campaign intervention`）を足すと本測定で漏れない。
