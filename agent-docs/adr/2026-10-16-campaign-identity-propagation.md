# ADR 2026-10-16: campaign 識別の伝搬（task_campaigns と決定ログ run 行）

- 状態: 採用（実装済み）
- 関連: 2026-10-15-decision-log-export, 2026-10-15-mlsys-min-*

## 文脈
決定ログの `campaign_id` / `arm` には書き手が無く、case・repetition・manifest hash・試行ごとの routing 設定は持てなかった。
全体 config の `[routing] mode` に頼ると B=fixed と C=decision_assisted を同時に比べられない。

## 決定
- D1: campaign の **root task にだけ** 識別（campaign_id / case_id / arm / repetition / manifest_hash / routing_setting）を持たせる。表は `task_campaigns`（migration 0085。統合時に 0084 task_routing_overrides の後へ振り直した）。
- D2: 子孫 task と全 run は行を持たず、`tasks.parent_id` の鎖を辿って最も近い祖先の識別を **読み出し時に解決** する（`task_campaign_resolve`）。子 task の生成経路（計画・委譲・replan・repair・直行）が増えても伝搬が落ちない。
- D3: 決定ログの書き込み（`write_decision_log`）が、task の全 run（初回・再試行・continuation・repair・reviewer・planner）の `decision_log_runs` へ同じ識別を書き、消費行の `campaign_id` も同じ値にする。再処理は run_id の置換で、行も合計も増えない。
- D4: 識別は人の API（`POST /tasks` の `campaign`）だけが付ける。CoS 経由の起票は 422。task 作成と同じ transaction で書く。不正な `routing_setting` は 400 で何も作らない。（注: API 欄の実装は範囲外のため本 branch には無く、別 unit で適用する。identity.md 参照）
- D5: campaign の無い task は行を持たず、run 行の識別は NULL のまま。
- D6: `routing_setting` は記録（監査）のみ。dispatcher が試行単位でこの値を routing mode として使う配線は本 ADR の範囲外（未実施。進捗ファイル参照）。
