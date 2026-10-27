# CoS 実 turn と人の介入の記録（campaign 計測の付記）

- 日付: 2026-10-16
- 関連: 2026-10-15-decision-log-export（D5〜D7）、2026-10-16-campaign-identity-propagation

2026-10-27 追記: D1 の usage 必須条件と D2 の未帰属 export 除外は
[CoS 消費の context と共有小計](2026-10-27-cos-usage-context.md) で置換した。

## 決定
- D1. CoS の実 turn は `chat_runs`（終端・`usage_json` あり）から `decision:cos:<run_id>:1` の消費行へ冪等に投影する。常に共有オーバーヘッド（`cos_turn_shared_overhead`）。観測できなかった欄は欠測のまま。費用は定価換算の欄だけ（実支出・quota に書かない）。
- D2. campaign への帰属は、turn の開始時刻に活動中（識別付与〜最後の活動）の campaign がちょうど 1 つのときだけ。0 個・複数は帰属なし（推測しない）。帰属なしの行も共有オーバーヘッドとして残るが campaign の export には入らない。
- D3. 人の介入は task の events から `<task_id>:<seq>` で冪等に投影する。`Answered`=research_judgment、`Retried`=recovery、`ApprovalDecided`・人の `Edited`=unclassified。所要時間は取れないので `wall_ms` は欠測。識別のある task だけ。
- D4. 投影は `record_task_decision_log`（dispatcher の run 完了後と通し試験が共通）で行う。LLM 呼び出しは無い。
- D5. 再投影で routing 側が書いた `proposed_first` / `proposed_candidates` を消さない（routing event が無い再投影の保護）。

## 未対応
- 手での復旧作業（daemon 再起動・DB 復旧）や手動 merge は event に残らないので自動では記録できない。評価者が最小の入力ログ（`decision_log_record_intervention`）を渡す。
