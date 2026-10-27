---
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
title: 判断・実行・成果・消費の最小一貫ログと campaign 単位 export（record schema・ラベル・消費の区分）
---
# ADR 2026-10-15: 判断・実行・成果・消費の最小一貫ログと campaign 単位 export

- 日付: 2026-10-15
- 状態: 採用（設計のみ。番号は振らない。実装は後続 unit `log-core` / `export-agg` / `log-wire` / `export-cli`）
- 親: `agent-docs/adr/2026-10-15-mlsys-min-decision-assisted.md` D3（最小ログ）の詳細化。D1 の優先順位・D4 の状態名は変えない。
- 関連: ADR-0069（routing 4 層）、2026-10-11-task-routing-features（`route-*` record）、ADR-0061/0072（`RunMetrics`）、ADR-0076（`QuotaUse`）、
  ADR-0090（クラスタ job 待ち）、2026-10-11-breaking-changes-by-default
- 進捗・調査: `agent-docs/progress/2026-10-15-mlsys-min/decision-log-design.md`

## Context

比較 A/B/C（fixed / Jev 分類 / …）の根拠は「何を提案し、実際に何を実行し、どう終わり、何を消費し、人が何をしたか」を 1 本の鍵で結べるログに依る。
既存には `routing_decided` event（`RoutingRecord`＋`RouteDecision`）、`route_decisions/outcomes` 表、`llm_proxy_requests` の相関欄、`WorkerStarted/Finished`（`Usage`・`RunMetrics`・`RunEnd`）、
`RoutingAudit`（run 単位の突き合わせ）、`ExecutionMetrics`（task 単位集計）がある。足りないのは (a) campaign の鍵、(b) 提案と実行の別欄の徹底、
(c) 3 階層の合否ラベル、(d) 消費の区分と欠測/推定、(e) 役割・介入の分類、(f) campaign 単位の export。新しい保管系は作らず、既存 event・表を再利用し欄を足す。
旧い仕組みは削除しない（`RoutingAudit`・`ExecutionMetrics` は残り、export が読み元として使う）。

## Decision

### D1. 対応キー（campaign / project / task / work-unit / run / decision）

1 件の判断・実行は次の鍵で結ぶ。欄名は export JSONL とストア欄で同じにする。

| 欄 | 型 | 意味・由来 |
|---|---|---|
| `campaign_id` | string | **新規**。campaign 単位の識別（`campaign` unit が発行。manifest hash と版を含む）。campaign 外の実行は null |
| `arm` | string? | **新規**。A/B/C などの比較腕。null = 腕なし |
| `project_id` | string | 既存（task の project） |
| `root_task_id` / `task_id` | string | 既存。`root_task_id` は ADR-0079 の木の根（export 時に導出） |
| `work_unit_id` | string? | 既存（`RoutingRecord.work_unit_id`）。task 直行は null |
| `run_id` | string | 既存。**1 run = 1 実行記録の主鍵**（冪等の軸、D3） |
| `decision_id` | string | 既存（`RouteDecision.decision_id`）。run への帰属は `RouteDecision.run_id` |
| `decision_stage` | enum | 既存 `RoutingStage` |
| `parent_run_id` | string? | **新規**。retry・continuation・repair の元 run（D6 の retry 分類に使う） |
| `attempt_no` | u32 | 既存 `RunMetrics.retries`（開始時点の `Task.attempts`）をそのまま使う |

campaign と task の結びは「campaign が作った root task 群」で持ち、run は task から辿る。run を直接 campaign に付けない（二重管理の防止）。

### D2. 提案と実行の別欄、除外・fallback・版

**提案**（判断時点。`RouteDecision` と `routing_decided` が既に持つ）と**実行**（実際に走ったもの。`WorkerStarted`＋`llm_proxy_requests` 相関欄）は別の接頭辞を持つ欄にし、
同じ名前で上書きしない。結果・消費は必ず**実行**欄に付き、`proposed_*` には付けない（最初の推奨候補への誤帰属の禁止）。

| 欄 | 区分 | 由来・備考 |
|---|---|---|
| `request_class` | 要求分類 | `TaskFeatures` の 9 水準（judgment…cross_cutting）＋ genre。分類の出し手は `classifier_kind`（`rule` \| `jev` \| `human_hint`） |
| `classifier_model` / `classifier_snapshot` | 分類 | Jev の指定 model ID と応答の日付つき snapshot。rule なら null |
| `classifier_confidence` | 分類 | D4。分類器自身の確信。**成功確率・propensity ではない** |
| `proposed_lane` | 提案 | `LaneDecision.proposed`（clamp 前）。`lane`（clamp 後）は `decided_lane` に名を分ける |
| `proposed_candidates[]` | 提案 | `RouteDecision.candidates`（source_id・model_id・harness_id・rank） |
| `proposed_first` | 提案 | `RouteDecision.selected`（最初の推奨）。これ**だけ**が提案の先頭 |
| `exec_source_id` / `exec_model` / `exec_account` / `exec_harness` / `exec_adapter` | 実行 | 実行した値。`WorkerStarted`（adapter・model・provider・account）＋`RoutingCorrelation.source_id`。実際に試した列は `RequestSourceAttempt[]`（`attempts`） |
| `exec_equals_proposed_first` | 派生 | 実行 = 提案先頭か。false なら理由が必須（下 2 行） |
| `exclusions[]` | 除外 | `RouteDecision.exclusions`（候補ごとの `ExclusionReason`） |
| `fallback_reason` | fallback | `RequestSourceAttempt.fallback_reason`（`rate_limit`・`health_down` など）。Jev 失敗の fixed 復帰は `classifier_fallback`（`timeout`/`rate_limit`/`server_error`/`bad_schema`/`unknown_class`/`circuit_open`/`credential_wait`） |
| `routing_mode` | 版 | **新規**。`fixed` \| `decision_assisted`（ADR D1） |
| `policy_version` | 版 | 既存 `LaneDecision.policy_version`／`RouteDecision.evidence_version` |
| `prompt_version` / `question_schema_version` | 版 | **新規**。Jev の state 組み立て規則と question 版（ADR D2） |
| `provider_model_version` | 版 | **新規**。応答が返す日付つき model 識別（記録のみ。pin と別） |
| `features_hash` / `options_hash` | 参照 | 既存（content address） |

既存の `provider`（アカウント含意）は新欄で `exec_account` に読み替える。ただし旧 `RoutingAudit.provider`・`WorkerStarted.provider` は削除せず読み元に残す（旧 event の読み取りのため。新規書き込み側は `exec_*`）。

### D3. ラベルの 3 階層、失敗系の記録、冪等キー

run 終了・WU 検証・campaign 合否は**別の欄**で、互いの代用にしない。値が未判定なら null（false にしない。既存 `RoutingOutcomeState` と同じ思想）。

| 階層 | 欄 | 値 | 由来・決める者 |
|---|---|---|---|
| run | `run_end` | `completed` \| `yielded` \| `budget_exhausted` \| `question` \| `failed` \| `harness_error` \| `cancelled` \| `waiting` \| `timeout` | `RunEnd`（既存）。`timeout` は `budget_exhausted{kind=wall}` または壁時計打ち切りを export で別名化（元の `RunEnd` も `run_end_raw` に残す） |
| run | `run_retryable` | bool? | `RunEnd::Failed.retryable` |
| WU | `wu_verdict` | `passed` \| `failed` \| `unverified` | 決定的 check／reviewer（worker の自己申告のみは `unverified`）。`RoutingOutcome.acceptance_passed`／`review_passed` から導出 |
| WU | `wu_failure_class` | string? | `RoutingOutcome.failure_class` |
| campaign | `campaign_verdict` | `pass` \| `fail` \| `unscored` | **新規**。独立 checker（campaign unit）が問題単位で付ける。再計算で一致するもの |

- 失敗・timeout・cancel・retry は**すべて 1 行ずつ残し分母から除かない**（`aggregate::build` の `error`/`cancelled` 除外は流用しない。除外数を出す場合も別欄 `excluded_count` を併記）。
- retry は新しい `run_id` を持つ別行で、`parent_run_id` と `attempt_no` で結ぶ。1 つの `decision_id` に複数 run が付き得るため、**結果の帰属は `exec_*`（その run が実際に走った候補）のみ**。
- **冪等キー**: 実行行の主鍵 = `run_id`。消費行 = `(run_id, usage_source)`、Jev など proxy 外呼び出し = `(decision_id, call_attempt)`、結果 = `outcome_id`（既存、`supersedes` で置換）。
  再処理（export の再実行・event 再投影・daemon 再起動後の再読み込み）は `INSERT ... ON CONFLICT(key) DO UPDATE`（最後の値で置換）で行い、**合算（`+=`）をしない**。集計は常にキー単位の最新 1 件を元に再計算する。
  `outcome` は `supersedes` に指された旧行を除外する（`latest_outcome` と同じ）。
- export の byte 一致のため、日付ではなく入力の最大 `seq`／`recorded_at` を `export_cursor` に出し、実行日を version に入れない。

### D4. Jev の confidence

- 保存名は `classifier_confidence`（0〜1、Jev の `confidence` そのまま）、各選択肢の確率は `classifier_probs`（`{option: p}`）。これは**分類の確信**。
- `success_probability`（LLM の成功見込み）と `propensity`（`RouteDecision.propensity` = 実行方策がその候補を選んだ確率）は**別の欄名のまま**。`classifier_confidence` から計算・代入・命名の流用をしない。
  `classifier_confidence` を `RoutingOutcome.reward` や evidence 表（`routing_evidence`）に書かない。export には 3 欄を別々に出し、`propensity` が無いもの（`decision_assisted` の Jev 決定）は null で、1.0 や confidence で埋めない。

### D5. 消費の区分（欠測 = null、推定 = estimated フラグ）

消費は 5 区分を別欄にし、区分間で加算しない。各数値欄は `Option` で、**欠測は null、0 は「0 だったと観測した」の意味**に限る。推定値は `<欄>_estimated: bool` を併記する。

| 区分 | 欄 | 由来 | 欠測・推定の扱い |
|---|---|---|---|
| token / cache | `input_tokens` / `output_tokens` / `cache_read_tokens` / `cache_creation_tokens` | `Usage`（既存） | 取れない harness は null。Jev は `usage` から |
| 定価換算 cost | `list_cost_usd` | `Usage.cost_usd`（`pricing` 静的単価表の推定）→ 欄名を明確化。常に `list_cost_usd_estimated=true` | 不明モデル・token 欠落は null（`cost_usd_complete=false` を run 集合に出す） |
| 実支出 | `cash_cost_usd` | `RoutingOutcome.cash_usd`、Jev の `usage.cost`（OpenRouter 応答の実請求）。サブスク枠は 0 でなく null | 実請求が取れなければ null。`list_cost_usd` で代用しない |
| quota | `quota[]`（source・account・window・`used_pct`・`method`） | `QuotaUse`（ADR-0076） | `unknown` は `used_pct=null`、`quota_unknown_runs` を併記。`free` のみ `Some(0.0)` |
| 計算資源（クラスタ待機） | `cluster_wait_ms`（job 投入→終了待ち）、`cluster_run_ms`、`cluster_node_hours`、`cluster_queue_ms` | ADR-0090 の `waiting` run・job event | 取れなければ null（`waiting` 区間の run 壁時計を代用しない。別欄 `wall_ms` と分ける） |
| 壁時計 | `wall_ms` | `RunMetrics.wall_ms`（run）。判断系は `decision_ms`（Jev 呼び出し） | — |

役割別・区分別・campaign 別の集計は、各欄について `sum`・`n_observed`・`n_missing`・`n_estimated` の 4 つ組で出す（和だけを出さない）。

### D6. 役割の分類

`role` は export 時に次の閉じた集合へ正規化する。既存 `RunRole`（worker/reviewer/planner）に、run でない消費を足す。

| `role` | 由来 |
|---|---|
| `cos` | CoS chat／escalation の run（既存の CoS セッション。task の run ではない消費は `source_kind=cos` で別行） |
| `planner` | `RunRole::Planner`（計画・replan） |
| `worker` | `RunRole::Worker`（`parent_run_id` が無い初回） |
| `reviewer` | `RunRole::Reviewer` |
| `jev` | decision model の呼び出し（`classifier_kind=jev`。`decision_id` ＋ `call_attempt` を鍵に proxy 外の消費として記録） |
| `retry` | `parent_run_id` を持つ再実行。**`role` は役割（worker/reviewer/planner）を保ち、`is_retry=true` と `retry_of` を足す**。集計の `retry` 列は `is_retry` の run の消費の和（役割別の和に重複しないよう、`by_role` と `by_retry` は別の表で出す） |

### D7. 介入の区分

人の介入は 1 件 1 行の `intervention`（event 由来。`ApprovalDecided`・answer・`handoff`・手動 `Transitioned` など既存 event から導出し、足りない分類だけ欄を足す）。

| `intervention_kind` | 意味 | 例 |
|---|---|---|
| `research_decision` | 研究の方針を人が決めた（campaign の結果に影響する選択） | decisions への回答、予算・測定の承認、受け入れ条件の選択 |
| `manual_handoff` | 人が作業を引き継いで実行した | 手で release 昇格、手で merge、手で実鍵登録 |
| `recovery` | 障害・不具合から戻すための人手 | daemon 再起動、DB 復旧、配送 repair の人手統合 |

欄: `intervention_id`（event の `seq` 由来で冪等）、`kind`、`task_id`、`campaign_id?`、`at`、`wall_ms?`（人の所要時間。取れなければ null）、`source_event`。
分類できない介入は `unclassified` として出し、推測で 3 区分へ入れない。

### D8. export の形式（campaign 単位）

`celerisctl campaign export <campaign_id> --out <dir>`（実装は `export-cli`）。出力は 2 ファイル:

- `records.jsonl` — 1 行 1 record、`record_type` で区別: `decision` / `run` / `outcome` / `usage` / `intervention`。各行に `schema`（`celeris.decision-log/1`）と D1 の鍵、D3 の冪等キー（`dedup_key`）を持つ。
  秘密（鍵・prompt 本文・command の生値）は出さず、`features_hash` などの参照と `secret id`・fingerprint のみ。
- `summary.json` — 集計。`campaign_id`・`export_cursor`・`manifest_hash`・`arms[]`、`by_role` / `by_cost_class` / `by_arm` の各表（D5 の 4 つ組）、`runs_by_end`（D3 の `run_end` 別件数。分母に失敗・timeout を含む）、
  `wu_verdicts`・`campaign_verdicts` の件数（3 階層を混ぜない）、`interventions_by_kind`、`cluster`（D5）、`incomplete`（欠測を含む区分の一覧）。

同じ入力からは byte 一致（キー順固定・時刻は入力由来）。同じ records を 2 回読み込んでも `summary.json` が変わらないことを試験で示す（D3）。

## 既存欄の再利用と追加欄

| 欄 | 既存の置き場 | 扱い |
|---|---|---|
| decision_id・stage・features_hash・options_hash・candidates・exclusions・selected・propensity・evidence_version | `RouteDecision`／`route_decisions`（`routing_decided` event） | **再利用** |
| lane・proposed・rule_id・policy_version・reasons・escalation | `LaneDecision`／`RoutingRecord` | 再利用。`lane` → `decided_lane` と読み替え（export 側） |
| work_unit_id | `RoutingRecord.work_unit_id` | 再利用 |
| source_id・model・account・decision_id・run_id | `llm_proxy_requests` 相関欄（`RoutingCorrelation`） | 再利用（実行側の正） |
| adapter・model・provider・account・role・task_role | `WorkerStarted` | 再利用 |
| wall_ms・retries・peak_context_tokens・turns | `RunMetrics` | 再利用 |
| tokens・cache・cost_usd | `Usage` | 再利用（`cost_usd` は export で `list_cost_usd` + estimated） |
| run_end | `WorkerFinished.end: RunEnd` | 再利用 |
| acceptance/review/failed_criterion_ids/failure_class/cash_usd/tokens/reward | `RoutingOutcome`／`route_outcomes` | 再利用（`reward` に classifier_confidence を入れない） |
| fallback_reason・attempts | `RequestSourceAttempt` | 再利用 |
| quota・quota_unknown_runs・cost_usd_complete | `ExecutionMetrics`／`QuotaUse` | 再利用 |
| run 単位の突き合わせ | `RoutingAudit`（純関数） | 再利用（export の run 行の読み元） |
| クラスタ待機の区間 | ADR-0090 の `waiting` run | 再利用（欄は追加） |
| `campaign_id`・`arm` | — | **追加**（campaign unit と共同。store は列 or 別表 `campaign_runs`） |
| `parent_run_id`・`is_retry` | — | **追加**（`WorkerStarted` に任意欄、または `route_decisions` 側） |
| `routing_mode`・`classifier_kind`・`classifier_model`・`classifier_snapshot`・`classifier_confidence`・`classifier_probs`・`classifier_fallback` | — | **追加**（`route_decisions.judge_json`／`RoutingRecord` の任意欄。`jev-adapter` と欄名を揃える） |
| `prompt_version`・`question_schema_version`・`provider_model_version` | — | **追加** |
| `wu_verdict`・`campaign_verdict` | `RoutingOutcome` の派生のみ | **追加**（`campaign_verdict` は新規、`wu_verdict` は導出。保存するかは `log-core` が決める） |
| `list_cost_usd_estimated`・`*_estimated`・`n_missing` | — | **追加**（export 側の派生。ストアに持たせない） |
| `cash_cost_usd`（Jev の実請求） | `RoutingOutcome.cash_usd`（run に紐づく分のみ） | 追加（proxy 外の `jev` 消費行: `decision_id`＋`call_attempt` 鍵） |
| `cluster_wait_ms` ほか | ADR-0090 の wait 区間 | **追加**（集計欄） |
| `intervention`（kind） | 既存 event（`ApprovalDecided` 等）から導出 | **追加**（分類欄。`unclassified` を許す） |

## Consequences

- 後続 unit は欄名をこの ADR に揃える。`log-core` が store の置き場（列か表）を決め、migration で既存 run・event を失わず移す（旧 event は新欄 null で読める）。
- `RoutingOutcome.reward` など既存欄に新欄の意味を混ぜない。変える場合は新しい ADR。
- 本番 config の変更は無い（`routing.mode` は親 ADR D1）。
- 範囲外: 独自ルータ学習、反実仮想評価（`propensity` は記録するが off-policy 評価はしない）。

## 付記（実装との突き合わせ, 2026-10-15）

- 実装済み: D1〜D4 の欄と冪等記録は `crates/task-core/migrations/0081_decision_log.sql`・`decision_log.rs`・`store/decision_log.rs`（run_id 主キーの置換。再処理・retry で行数も合計も増えない）。dispatcher からの書き込みは `task-dispatch/src/dispatcher/decision_log_wire.rs`（events から決定的に投影、LLM 呼び出しなし）。D5〜D8 の集計と export は `task-core/src/decision_log_export.rs`、CLI は `celerisctl campaign export <campaign-id> --out <dir>`（`records.jsonl` + `summary.json`、秘密は `[redacted]`、同一 DB から byte 一致）。
- 提案（`proposed_*`）と実行（`exec_*`）は別欄で、結果・消費は実行側のみに付く。run 終了・WU 検証・campaign 合否は別ラベル。この配線は campaign 合否を書かない（既存値を保持）。
- 本文からの差: `export_cursor` は None のまま。cost は定価換算（estimated）で、実支出 `cash_cost_usd` は欠測のまま（定価で代用しない）。
- 未実装（後続）: `campaign_id`・`arm`・`routing_mode`・Jev classifier 欄の書き手（欄と保持は実装済み、書く側は campaign／Jev adapter unit）。CoS・cluster 待機・介入の消費行の自動書き込み。reviewer run の判断 record。この間 `campaign_id` が None の run は export 対象外。
- 未検証: 実 LLM を使った測定は行っていない（試験は決定的な fixture のみ）。

## 付記（2026-10-27、介入の主体）

D7 の介入に主体（`actor_kind`・`actor_id`）を足し、`DecisionAnswered`・`IntegrationAnswered`・`PlanGateDecided` を写像に
加えた。summary の `interventions_by_kind` は `human_interventions_by_kind`（人だけ）と `interventions_by_actor_kind`
（全主体）に置き換えた。wait に `decision` の区間を足した。詳細は
[2026-10-27-decision-log-intervention-actors](2026-10-27-decision-log-intervention-actors.md)。
