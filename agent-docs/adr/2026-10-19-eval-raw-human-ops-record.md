# 多軸評価の生記録: A/B 共通の人の操作・経過時間・やり直し（human-ops JSONL）の形式と identity・時計の規則

- 日付: 2026-10-19
- 状態: 採用（実装済み: `crates/campaign/src/human_ops.rs`・`human_ops_join.rs`、評価者 runbook は `docs/ops/eval-human-ops-runbook.md`）
- tasks: [01M59KXY854W5C65GTG9GVQVAC]
- 関連: 2026-10-15-decision-log-export、2026-10-16-campaign-identity-propagation、2026-10-16-campaign-repeats-denominator、
  2026-10-16-campaign-comm-failure-tracking、ADR-0090（cluster job wait）
- 定義書（フィールド表・操作種別表・event 由来/評価者記録の区分表・限定事項）: `docs/ops/eval-raw-record-definitions.md`

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
- D9（実装の置き場所）: 型・検証・冪等追記は `crates/campaign/src/human_ops.rs`、CLI（`celeris-campaign human-op` の record/list/validate）は
  同 `mod cli`、event/run との結合は `human_ops_join.rs`、評価者の手順は `docs/ops/eval-human-ops-runbook.md` が本 ADR に従う。
  形式を変えるときは `schema` の版を上げ、本 ADR に付記する。
- D10（live 記録の再開）: 開始時の `time_basis` を記録に固定する。`--started-at now` で始めた区間を明示時刻の `--ended-at` で
  終える追記（op_end）は、開始行の本体と比べず、`time_basis`・終了時刻は同一性の比較欄から外す。開始行は書き換えない。
- D11（join の入力）: join は新しい出力形式を足さず既存の出力を再利用する。runner の out dir の `trials.jsonl`・`ledger.jsonl` と、
  decision-log export の records を入力にする。`celeris-campaign export` の table/final-accounting は join の入力ではない。


## 付記（2026-10-27）

schema を `celeris.eval-human-op/2` に上げた（D2・D9）。主体の種類 7 種・`decided_by`/`executed_by`/`approved_by`・`credential`・
`request_ref`/`request_revision`（record_id の材料）・`policy_ref` を足し、report は介入を論理要求単位・主体別に集計する。
決定は [2026-10-27-human-ops-actor-roles-request-ref](2026-10-27-human-ops-actor-roles-request-ref.md)。/1 の行は読まない。
