# 多軸評価の生記録: A/B 共通の人の操作・経過時間・やり直し（human-ops JSONL）の形式と identity・時計の規則

- 日付: 2026-10-19
- 状態: 採用（形式の固定のみ。実装は後続 WU: core → cli → join → runbook）
- tasks: [01M59KXY854W5C65GTG9GVQVAC]
- 関連: 2026-10-15-decision-log-export、2026-10-16-campaign-identity-propagation、2026-10-16-campaign-repeats-denominator、
  2026-10-16-campaign-comm-failure-tracking、ADR-0090（cluster job wait）
- 定義書（フィールド表・操作種別表・event 由来/評価者記録の区分表・限定事項）: `docs/ops/eval-raw-record-definitions.md`

## 背景
MLSys 多軸評価（主比較 A0=直接 Claude Code+Sonnet / B0=Celeris+Sonnet、Jev 副比較 B1/C1）は、人の調整・復旧と
やり直しが減るかを主張したい。既存の仕組みで取れるのは次の範囲だけで、**人の操作の所要時間と A（Celeris を通らない arm）の
人の操作は何も取れない**。

| 既存（main c9654bff で確認） | 取れるもの | 足りないもの |
|---|---|---|
| `crates/campaign/src/record.rs` `TrialRecord`（`trials.jsonl`） | 試行の identity（split/config/problem_id/trial_no/rep）、outcome、`wall_secs`、tokens、observed run | 人の操作。`wall_secs` は `u64` で、未実行（`ran()==false`）と `executor_error` は **0**、timeout 超過は timeout に丸める（`runner.rs`） |
| `crates/campaign/src/ledger.rs` `LedgerRow`（`ledger.jsonl`） | `TrialKey`（manifest_sha256/problem_id/arm/trial_no/rep）と `idempotency_key()`=`campaign-<sha256>`、task_id/run_id/child_task_ids/cluster_job_ids、`started_at`/`submitted_at`（epoch 秒、旧行は None）、通信失敗 | 人の操作、外部待ちの内訳 |
| `crates/campaign/src/manifest.rs` `CampaignManifest` | campaign_id、sha256、`ConfigId` A/B/C、`repeats` | 実験セット（A0/B0 と B1/C1 の区別）の欄は無い |
| `crates/campaign/src/final_accounting.rs` | trials と decision-log export の campaign/case/arm/repetition/hash 結合、欠測を 0 にしない集計規約 | 時間・人の操作は対象外 |
| `task-core` `decision_log_project_interventions`（`store/decision_log.rs`）・`decision_log_export.rs` | Celeris の人 event（Answered→research_judgment、Retried→recovery、ApprovalDecided/Edited(by=human)→unclassified）を `intervention_id=<task_id>:<seq>` で冪等に投影。**`wall_ms` は常に None** | 所要時間、A の介入、safety と system の区別 |
| `task_campaigns` / `task_campaign_resolve`（ADR 2026-10-16-campaign-identity-propagation） | root task の campaign_id/case_id/arm/repetition/manifest_hash、子孫は祖先から解決 | — |
| events（`Transitioned`・`WorkerStarted/Finished`・`ClusterJobWait*`・`QuestionRaised`/`Answered`・`ApprovalRequested`/`Decided`・`BrowserWait*`・`Retried`・`RepairScheduled`）と `runs`（started_at/finished_at/seq） | B/C の状態区間・run 区間・外部 job 待ち・人待ちの時刻点 | A は events/runs を持たない |
| `crates/routing-eval` | routing 評価の集計 | 人の操作・時間の欄は無い（再利用先にしない） |

## 決定
- D1（取り直さない）: events/runs/TrialRecord/ledger/intervention projection から導出できる量は生記録に**書かない**。
  生記録（評価者が書く JSONL）は、既存の仕組みが原理的に取れない事実だけを持つ:
  (a) 人（と評価者が動かした script）の操作の開始・終了・種別・意図・理由・対象、
  (b) A arm の外部 job の scheduler 計上値と人待ち区間（Celeris の event が無いため）、
  (c) 観測打ち切り（observation_cutoff）、(d) 評価者が付ける論理作業（logical_op）と attempt。
  どの量をどこから取るかは定義書の「区分表」で固定する。
- D2（置き場所と schema）: campaign の out ディレクトリ（`trials.jsonl`・`ledger.jsonl` と同じ場所）に `human-ops.jsonl`、
  1 行 1 record、`schema = "celeris.eval-human-op/1"`。本番 SQLite には書かない（ledger と同じ）。追記専用。訂正は
  新しい行（`record_type = correction`、`corrects = <record_id>`）で表し、既存行を書き換えない。
- D3（identity）: 既存 campaign の ID をそのまま使い、新しい ID 体系を作らない。
  `campaign_id`=`CampaignManifest.campaign_id`、`manifest_sha256`=`CampaignManifest.sha256`=`TrialKey.manifest_sha256`=
  `CampaignIdentity.manifest_hash`、`case_id`=`problem_id`、`arm`=`ConfigId`（`"A"|"B"|"C"`）、`repetition`=`rep`、
  `trial_no`、`trial_key`=`TrialKey::idempotency_key()`、`task_id`/`run_id`=Celeris の ID（A は null）、
  `job_ref`=`<scheduler>:<cluster>:<job_id>`（`celeris_exec.rs` の `job_ref` と同じ形）。
  新規は `experiment_set`（`main-ab` | `jev-bc`）だけで、表示名 A0/B0/B1/C1 は `(experiment_set, arm)` から導出する
  （`main-ab` は A/B、`jev-bc` は B/C のみ許す。それ以外の組は検証エラー）。両セットは別 campaign_id・別 out。
- D4（安定 record_id）: `record_id = "hop-" + hex(sha256(正規化 JSON))`。正規化 JSON は「同じ事実か」を決める欄だけ
  （定義書の「record_id の材料」）を key 順に並べたもので、`recorded_at`・`note`・`host` は含めない。
  同じ record_id の再追記は、材料外を含む本体が同じなら no-op（行を増やさない）、違えば conflict で拒否する（上書きしない）。
  再開・再送・二重実行で行が増えないこと、読み出しは record_id で 1 件に潰すこと（`decision_log_export` の `intervention_id` と同じ規約）。
- D5（時計）: 各行に `recorded_at`（書いた時刻、RFC 3339 UTC、CLI の host の時計）、`clock_source`、`host` を必ず持つ。
  事実の時刻（`occurred_at` / 区間の `start`/`end`）は別欄で、評価者の申告・scheduler 計上・event の ts などの出所を
  `time_basis` に書く。時刻・所要時間が分からない欄は **null** で、`missing` に理由（`unknown`/`not_recorded`/
  `not_applicable`/`open_at_cutoff`）を書く。**0 を「分からない」の意味に使わない**。0 は観測した 0 だけ。
  既存の `TrialRecord.wall_secs` の 0（未実行・executor_error）も結合時に欠測へ戻す（定義書 §4）。
- D6（操作種別）: `op_kind` は閉じた列挙（定義書の表）。研究方針判断（`research_direction`）・安全承認（`safety_approval`）・
  システム都合の調整（`system_adjustment`）と復旧（`system_recovery`）を別の値にする。`mode`（manual/automatic）と
  `intent`（research/safety/system/protocol/monitoring）を別欄に持つ。分類に迷うものは `unclassified`（推測で寄せない。
  intervention projection と同じ方針）。A と B は同じ表・同じ判定規則で記録する。
- D7（A/B 同一性）: 人の操作は **A でも B でも評価者が生記録に書く**（B の Celeris event は照合用で、加算しない）。
  B の event から導出した介入と評価者の記録は `celeris_event_ref` で突き合わせ、片方にしか無いものは不一致として報告する。
  外部 job の待ちは A/B とも scheduler 計上値（`job_observation`）を主に使い、B の `ClusterJobWait*` は照合用。
- D8（時間・やり直し）: 区間は submitted/started/ended/observation_cutoff を端点に、実行・ready_not_running・
  external_queue_wait・human_wait・retry/recovery を定義書 §5 で定義する。同じ論理作業の再実行は同じ `logical_op_id` で
  `attempt` を 1 から数える。observation_cutoff で終わっていない区間は打ち切り（censored）として null＋`open_at_cutoff`。
- D9（範囲）: 本 ADR は形式と規則だけを固定し、コードは書かない。型・検証・冪等追記は core WU、CLI（record/list/validate）は
  cli WU、event/run との結合は join WU、評価者 runbook は runbook WU が本 ADR に従って実装する。形式を変えるときは
  `schema` の版を上げ、本 ADR に付記する。

## 結果
- 既存の記録を二重に持たない。人の操作の所要時間と A の人の操作が、初めて B と同じ定義で数えられる。
- 主張の限定（記録できない指標）は定義書 §6 に固定し、報告はそこを引用する。
- 評価者の手入力に依存するため、評価者が気付かない操作は抜ける（§6）。抜けを 0 として数えない。
