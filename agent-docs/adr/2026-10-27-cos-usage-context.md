# CoS 消費の thread・案件・session と共有小計

- 日付: 2026-10-27
- 状態: 採用
- task: 01M5XXF7Q7VE9G6Q3S83CA9ZBE

## 決定

1. `DecisionLogUsage` と export の usage 行に `thread_id`・`thread_kind`・`project_id`・`model`・`session` を付ける。CoS は既存の冪等キーから chat_runs を特定し、chat_threads と、その run の session_row_id が指す node_sessions を読み出し時に結合する。新しい保存系や migration は不要で、過去の消費行にも適用する。model は run の resolved_config_json.model を優先し、無ければその session の model。session は node_sessions.session_id（空文字は null）。現在の active session への付け替えはしない。legacy thread は legacy のまま、不明は null とする。
2. CoS は案件が分かっても試行・case への直接帰属ではない。共有のまま保持し、`shared_overhead_by_kind` を `cos:general` / `cos:project` / `cos:legacy` / `cos:unknown`（他の role は role 名）で集計する。総小計と内訳は別の切り口で、加算しない。試行へ均等配分しない。
3. campaign の活動窓による既存の一意帰属は維持する。campaign 未帰属の共有行も campaign export に含め、元の campaign_id=null を維持する。これは全体の未帰属共有消費であり、その campaign の費用ではない。別 campaign に明示帰属する行は含めない。複数 export を併合するときは dedup_key で重複を除く。
4. usage_json が無い終端 run も投影する。数値は null、観測された 0 は 0。壊れた usage_json も観測不能として残す。進行中 run は対象外。既存の `usage_json IS NOT NULL` による除外を削除する。

## 関連

- [CoS turn 記録](2026-10-16-cos-turn-intervention-recording.md) D1 の usage 必須条件と D2 の未帰属 export 除外を本決定で置換する。
- [計測の帰属](2026-10-16-metering-attribution.md) の欠測・共有分離を維持する。
