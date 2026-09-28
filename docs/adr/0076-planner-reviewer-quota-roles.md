# ADR-0076: planner / reviewer run も quota の観測対象にし、`QuotaUse` に役割別の run 数を足す

- 日付: 2026-09-28
- 状態: Accepted（F5-1 dogfood 4 回目の WU `quota-roles` で実装）
- 関連: ADR-0074 D4（quota の観測と記録）と「Phase F3 実装時の逸脱・明確化」4（quota の対象は worker/WU の run だけ）。
  本 ADR はその 4 を解消する。ADR-0074 の本文は書き換えない。ADR-0014 D1（`RunRole`、Reviewer run の `WorkerStarted`）、
  ADR-0072 D14（task-local な planner run）。

## 1. 文脈

- ADR-0074 F3 では `Event::QuotaEstimated` を worker / WU の run にだけ出した。planner run は完了が `on_planner_finished` に、
  Reviewer run は `on_review_finished` に分岐し、worker 用の `QuotaActivity::end` を通らないため、`quota_begin` 自体を呼ばずに
  済ませていた。
- その結果、同じアカウントで planner / reviewer が worker と重なって走っても、`QuotaActivity` の重なり判定に現れず、
  worker の `measured` が planner / reviewer の消費を含んだまま「単独」と判定されうる。`ExecutionMetrics.quota` にも
  planner / reviewer の消費が載らない。

## 2. 決定

### D1. planner / reviewer run も同じ `QuotaActivity` を通す

- planner run: dispatch で worker と同じく `quota_begin` を呼ぶ。`on_planner_finished` は `usage` を決めた直後、計画の検証・
  採用の分岐より前に `resolve_quota_estimate` を 1 回だけ呼び、返った `QuotaEstimated` を `WorkerFinished{role: planner}` を
  保存するすべての分岐（採用・再試行・諦め・部をまたぐ子の質問）で同じトランザクションに添える。
- Reviewer run: `WorkerStarted{role: reviewer}` と runs 索引の開始を書いた直後に `quota_begin`。`on_review_finished` は
  `ReviewEntry` の provider / account と `WorkerStarted.model` で、対象タスクの取得より前に `resolve_quota_estimate` を 1 回だけ
  呼ぶ（対象タスクが消えていても bookkeeping は閉じる）。`WorkerFinished{role: reviewer}` を残す 3 経路（stale・供給側失敗の延期・
  通常の判定）すべてにその Event を添える。cancel 等でレビューを止めたときは `release_quota_if_tracked` で閉じる（Event は残さない）。
- Reviewer run を起こさないレビュー（command / artifact_exists / human だけ）は run が無いので Event を作らない。
- アカウントプールを使わない run は、worker と同じく `QuotaActivity` に登録せず、終了時に `free` の Event を 1 件出す（D4.2 手順 5）。

### D2. 役割は `WorkerStarted.role` から導き、Event の形は変えない

- `Event::QuotaEstimated` に role を足さない。集計（`task_core::execution_metrics`）は events の `WorkerStarted` から
  run_id → role を引いて join する。`role` の無い `WorkerStarted`、または `WorkerStarted` が見つからない run
  （旧 Event・別タスクへ書かれた按分の再送）は **worker** として数える。既存の `runs_by_role`（`ExecutionMetrics` の run 数）と
  同じ規則。

### D3. `QuotaUse.runs_by_role`

- `QuotaUse` に `runs_by_role: BTreeMap<String, u32>`（キーは `"worker"` / `"planner"` / `"reviewer"`）を足す。
  `runs`（総数）は従来どおりで、`runs_by_role` の値の合計と一致する。`#[serde(default)]` なので、欄の無い旧 JSON も読める
  （空 map）。`aggregate_quota_use` は record の role で、`merge_quota_use` は map の和で内訳を合算する。
- `task_core::quota::QuotaRunRecord` に `role: RunRole` を足す（集計の入力。永続化しない）。
- API schema（`docs/api/v1/api-v1.schema.json`）と GUI の生成型はこの欄の追加に合わせて再生成する（追加のみ。必須にしない）。

## 3. 帰結

- planner / reviewer と worker が同じアカウントで重なれば apportioned になり、単独の worker の measured が他の役割の消費を
  含むことが無くなる。
- `GET /tasks/{id}/metrics` / `GET /metrics/execution` の quota 行で、どの役割がどれだけの run を占めたかが読める。
- quota は引き続き観測と記録だけ（ADR-0074 D4 の制約）。dispatch の判断には使わない。LLM 呼び出しは足さない。
