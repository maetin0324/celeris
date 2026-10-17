# P1-6 final-accounting: 実 router 経由の最終集計の検証記録

---
tasks: [01M54N11F9PWWD39MJ4J1BBJG5]
---

campaign の最終報告の総消費量（外側 trial・run・Jev・CoS・共有）を、実 task-api router 経由で積んだ記録から再生成して検証した記録。

## 修正 commit

- 本 task の commit（`git log --oneline -1 -- crates/campaign/src/final_accounting.rs` で確認）: `unfinished_rows`（`crates/campaign/src/final_accounting.rs`）が台帳 `ledger.jsonl` の**全履歴行**から非終端行を数えていた。台帳は状態遷移ごとに行を追記するので、全 trial が done でも途中状態の行が予約として数えられ、通し試験で `ledger_unfinished: 21 ledger row(s)`・`reservation_tokens_not_in_final=6_300_000` と誤報告された（3 trial × 7 遷移 × per_case 300_000）。鍵ごとに最後の行だけを見るように直した（`Ledger::open` と同じ last-wins）。
- 追加試験: `crates/celeris/tests/campaign_acceptance_e2e.rs` に 2 件（下記）と、共有 helper `project_decision_log`（既存の通常経路試験から抜き出し）。

## 既存の修正を省いた根拠

既存 writer（run/usage の投影 `record_task_decision_log`、Jev/CoS の `decision_log_record_usage`、export `decision_log_export_campaign`、集計 `final_accounting::compute`）は再実装・変更していない。試験は既存 API だけを使い、識別・使用量を境界の fake JSON や直接 SQL で補っていない。上の台帳走査以外に不一致は見つからなかったため、他の writer・DTO・schema は触っていない。

## 試験

- `final_accounting_is_regenerated_from_the_router_tree_with_jev_cos_gaps`: 実 HTTP の task-api・一時 DB・内部 stub の run で B/C の木（planner 50 + 子 300 + worker 200 + reviewer 20 の input と各 output 1）を作り、A（harness 試行 15 token、run 内訳なし）と合わせた外側合計 1163 から再生成。
  1. Jev/CoS 行なし → `observed_sum.tokens=1163`、`total=null`、削減 `reduction_confirmable.tokens=false`（`cos_not_retrieved`・`jev_unknown`）。
  2. Jev 行はあるが tokens 欠測、CoS 呼び出しなしの根拠あり → `total=null`・削減未確定（`metric_missing`）。
  3. Jev tokens 2+3 を観測 → `total.tokens=1168`・削減確定。list/cash 費用は A の内訳なしで `null`（0 にしない）。
  4. 再投影・再 export・再集計・records の二重投入で total は同一（`again == done`、jsonl 同一）。
- `final_accounting_shows_budget_view_versus_final_cutoff_after_a_budget_stop`: 予算停止（max 100000、停止 trial 80000）で `runtime_view.charged=80067`、`final_cutoff.observed=80067`、差 0、予約残 0。
- budget の数値: 台帳に未完了行があるときの `reservation_tokens_not_in_final`（予約は最終に入らない）の差は、完了済み campaign では 0 であることを固定した（修正前は 6_300_000 と誤っていた）。

## 実行したコマンドと結果

- `cargo test -p celeris --test campaign_acceptance_e2e && cargo test -p celeris --test campaign_wiring_e2e && cargo test -p celeris-campaign --test final_accounting`: exit 0（9 / 1 / 11 passed）
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5635、failed 0、ignored 15）
- `cargo clippy --workspace -- -D warnings`: exit 0、`cargo fmt --all -- --check`: exit 0
- ログ: run の artifacts ディレクトリ（`acceptance0.log`、`test-parallel.log`、`clippy.log`）。範囲外の失敗検査はなし。

## 再生成手順

1. `decision_log_export_campaign`（または `celerisctl` の campaign export）で `records.jsonl` を作る。
2. `celeris-campaign final-accounting --manifest <m> --out <dir> --records records.jsonl [--cos-not-called <根拠>] [--quota q.json]`、または `export --records …`（表の `consumption` と `final-accounting.json`）。試験は CLI と同じ `compute` → `recheck` → `export_with` を直接呼ぶ（bin は別 crate で試験から起動できないため。CLI 引数の解釈自体は通していない）。

## token / 費用 / quota の定義

- token は manifest の `token_basis`（`all` = input + output + cache_read + cache_creation）。内訳は別に出す。
- 定価換算（list）・実支出（cash）・executor 報告費用は別項目で、互いに代用・加算しない。欠測は 0 にせず `null` と gap。
- quota は消費行に載らず別項目（未観測なら `not_observed`）。
- 外側 trial の合計と decision-log の run は照合のみで加算しない。retry は `by_role` の同じ費用の別の切り口。

## 区分

| 項目 | 区分 |
|---|---|
| 最終集計の結合・欠測・重複排除・再生成 | 実装済み / 実経路検証済み（実 task-api router・正式 DTO・一時 DB・既存 writer/projection・export） |
| worker/planner/reviewer の run イベント、Jev/CoS 行、成果物 | mock 検証済み（試験が内部 stub として積む。Jev/CoS は既存 store API 経由） |
| 実 dispatcher・実 LLM・実 CoS セッション・本番 DB・実 quota | **未検証** |
