# 多軸評価の生記録（human-ops JSONL）の定義: identity・時計・人の操作種別・時間とやり直し・限定事項

---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
---

根拠: ADR `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md`、/2 の主体・役割・論理要求は ADR `agent-docs/adr/2026-10-27-human-ops-actor-roles-request-ref.md`。この文書は **測定開始前に固定する定義** で、
評価者は A（A0）と B（B0）、および Jev 副比較（B1/C1）で同じ定義を使う。実装（型・CLI・結合）はこの定義に従う。
形式を変えるときは `schema` の版を上げ、ADR に付記する。

## 1. ファイルと行

- 場所: campaign の out ディレクトリ（`trials.jsonl`・`ledger.jsonl` と同じ）に `human-ops.jsonl`。
- 1 行 = 1 record（UTF-8 JSON、改行区切り）。`schema = "celeris.eval-human-op/2"`。/1 の行は読み込みで拒否する（変換しない）。
- すべての欄は JSON に存在しなければならない（その型で使わない欄は null）。欄の無い行は形式エラー。
- 追記専用。訂正は `record_type = "correction"` の新しい行で、元の行は書き換えない。
- `record_type` の値:

| record_type | 意味 | 主な欄 |
|---|---|---|
| `op_start` / `op_end` | 人（または評価者が動かした script）の操作の開始・終了 | §3 の op 欄 |
| `wait_start` / `wait_end` | 人待ち（human_wait）の開始・終了。**A のみ**（B は event から導出、§5） | `wait_kind`、`logical_op_id` |
| `job_observation` | 外部クラスタ job の scheduler 計上値（submit/start/end）。A/B 共通 | `job_ref`、`job_submit`/`job_start`/`job_end` |
| `attempt` | 論理作業の 1 回の試み（開始・終了・結果）。Celeris の run を持たない作業（A 全部、B の人の作業）に使う | `logical_op_id`、`attempt`、`start`/`end`、`result` |
| `cutoff` | 観測打ち切り（その試行の観測をここで止める） | `occurred_at` |
| `correction` | 既存行の訂正 | `corrects`、置き換える欄 |

## 2. 共通欄（全 record）

### 2.1 identity（既存 campaign の ID と対応）

| 欄 | 型 | 必須 | 既存の対応先 | 規則 |
|---|---|---|---|---|
| `schema` | string | 必須 | — | `celeris.eval-human-op/2` |
| `record_id` | string | 必須 | — | §2.3。`hop-<sha256 hex>` |
| `record_type` | enum | 必須 | — | §1 の表 |
| `experiment_set` | enum | 必須 | （新規） | `main-ab`（A0/B0）または `jev-bc`（B1/C1）。両セットは別 campaign_id・別 out |
| `campaign_id` | string | 必須 | `CampaignManifest.campaign_id` / `task_campaigns.campaign_id` | — |
| `manifest_sha256` | string | 必須 | `CampaignManifest.sha256` = `TrialKey.manifest_sha256` = `CampaignIdentity.manifest_hash` | 正規化後の manifest の hash（`campaign.json` の値） |
| `case_id` | string \| null | 試行に属するなら必須 | `TrialRecord.problem_id` / `CampaignIdentity.case_id` | campaign 全体の操作（環境復旧など）は null |
| `arm` | `"A"`\|`"B"`\|`"C"` \| null | 同上 | `ConfigId` / `CampaignIdentity.arm` | `main-ab` は A/B、`jev-bc` は B/C のみ。他の組は検証エラー |
| `repetition` | u32 \| null | 同上 | `TrialKey.rep` / `CampaignIdentity.repetition` | 1 始まり |
| `trial_no` | u32 \| null | 同上 | `TrialKey.trial_no` | — |
| `trial_key` | string \| null | 同上 | `TrialKey::idempotency_key()`（`campaign-<hex>`）= `LedgerRow.idempotency_key` | 結合の主キー。identity 欄と矛盾したら検証エラー |
| `task_id` | string \| null | 任意 | Celeris task（`LedgerRow.task_id`・子 task） | A は常に null（`missing.task_id = not_applicable`） |
| `run_id` | string \| null | 任意 | Celeris `runs.run_id` / `LedgerRow.run_id` | A は null（not_applicable） |
| `job_ref` | string \| null | 任意 | `<scheduler>:<cluster>:<job_id>`（`celeris_exec.rs` の `job_ref`、`LedgerRow.cluster_job_ids`） | 同じ job は A/B で同じ書式 |
| `logical_op_id` | string | op/wait/attempt で必須 | （新規。評価者が付ける） | 1 つの論理作業（例 `p3/submit-train`）。やり直しは同じ id で `attempt` を増やす |
| `attempt` | u32 | op/attempt で必須 | B の Celeris run は `runs.seq`（導出） | 1 始まり。同じ `logical_op_id` 内で連番 |

表示名（`arm_label`）は記録せず導出する: `(main-ab, A)=A0`、`(main-ab, B)=B0`、`(jev-bc, B)=B1`、`(jev-bc, C)=C1`。

### 2.2 時計

| 欄 | 型 | 必須 | 規則 |
|---|---|---|---|
| `recorded_at` | RFC 3339 UTC string | 必須 | その行を書いた時刻（CLI が自 host の時計で入れる。評価者は書かない） |
| `clock_source` | enum | 必須 | `recorded_at` の時計: `system_utc`（host の時計、同期状態は未確認）/ `system_utc_ntp_synced`（書く時点で NTP 同期を確認できた）/ `unknown` |
| `host` | string | 必須 | `recorded_at` を取った host 名（分からなければ `"unknown"`、空文字にしない） |
| `occurred_at`・`start`・`end` 等の事実時刻 | RFC 3339 UTC string \| null | 型ごと | 分からなければ null（§2.4） |
| `time_basis` | enum | 事実時刻があれば必須 | 事実時刻の出所: `evaluator_live`（その場で CLI を叩いた＝recorded_at と同じ）/ `evaluator_reported`（後から申告）/ `harness_transcript`（Claude Code 等の transcript の ts）/ `scheduler_accounting`（sacct・qstat の計上値）/ `celeris_event`（event の ts）/ `unknown` |
| `time_precision` | enum | 事実時刻があれば必須 | `second` / `minute` / `approximate`（申告で幅がある） |

### 2.3 record_id（安定 ID）の材料と冪等規則

- `record_id = "hop-" + hex(sha256(canonical_json(材料)))`。`canonical_json` は key を辞書順に並べた空白なしの JSON
  （`manifest.rs` の `canonical` と同じ並べ方）。
- 材料 = `schema`, `record_type`, `experiment_set`, `campaign_id`, `manifest_sha256`, `case_id`, `arm`, `repetition`,
  `trial_no`, `logical_op_id`, `attempt`, `op_kind`, `wait_kind`, `job_ref`, `corrects`, `request_ref`, `request_revision`
  （無い欄は null として入れる）。同じ論理作業・attempt でも revision が違えば別の行。
  `op_start`/`op_end` と `wait_start`/`wait_end` は `record_type` が違うので別 id。同じ論理作業・同じ attempt の同じ種別の
  開始は 1 件しか存在しない。
- 材料に入れない: `recorded_at`, `clock_source`, `host`, `time_basis`, `time_precision`, `note`, 時刻の値そのもの。→ 同じ事実を再送・再開・二重実行で
  書き直しても id は変わらない。
- 追記時: 同じ id が既にあり、本体（材料外の欄で `recorded_at`/`clock_source`/`host`/`time_basis`/`time_precision` を除いたもの。先に書いた行の時計注記を保つ。live 開始 → 明示時刻の終了追記で開始行の再送が拒否されないため）が同じなら **no-op**
  （行を増やさない）。違えば **conflict で拒否**（黙って上書きしない）。直したいときは `correction` を書く。
- 読み出し時: record_id で 1 件に潰し、`correction` は `corrects` の行の欄を置き換える（訂正の訂正は最後の行が勝つ。
  順序は file の行順）。並行追記は 1 行 1 write（O_APPEND）＋fsync とし、読み出し側の潰しで重複を吸収する。

### 2.4 欠測（Option）規則 — 未知の時間を 0 にしない

1. 時刻・所要時間・回数のうち **分からないものは null**。欄そのものは省略しない（省略は形式エラー）。
2. null の欄は `missing` オブジェクトに理由を書く: `{"<欄名>": "unknown" | "not_recorded" | "not_applicable" | "open_at_cutoff"}`。
   - `unknown`: 起きたが時刻が分からない（例: 気付いたときには終わっていた操作の開始）。
   - `not_recorded`: 記録し忘れた・記録手段が無かった。
   - `not_applicable`: その arm・種別に存在しない（A の `task_id`、A の `ready_not_running`）。
   - `open_at_cutoff`: observation_cutoff の時点で終わっていない（打ち切り）。
3. **0 は「観測した 0」だけ**。0 を書く欄は `time_basis` を持たなければならない。所要時間は `end - start` から読み出し時に
   計算し、記録しない（両端のどちらかが null なら所要時間も null）。
4. 集計は `decision_log_export` の `Stat` と同じ 4 つ組（和・観測数・欠測数・推定数）。観測 0 件の和は null。欠測が 1 件でも
   あれば「合計」は出さず `observed_sum` と欠測数を並べる（`final_accounting` と同じ規約）。
5. 開始だけ・終了だけの op は捨てない。開始だけ → 終了 null（cutoff 後なら `open_at_cutoff`、前なら `not_recorded`）、
   終了だけ → 開始 null（`unknown`）。どちらも件数には数え、所要時間は欠測。
6. 既存記録の 0 も結合時に欠測へ戻す: `TrialRecord.wall_secs` は `ran()==false`（budget_stop・dependency_blocked・未実行の
   arm_budget）なら `not_applicable`、`reason == executor_error` なら `unknown`。timeout 超過の試行は timeout に丸めた値
   （下限）として `time_precision = approximate` 扱い。`LedgerRow.started_at`/`submitted_at` の None（旧行）は `not_recorded`。

## 3. 人の操作（op_start / op_end）

### 3.1 欄

| 欄 | 型 | 必須 | 規則 |
|---|---|---|---|
| `op_kind` | enum | 必須 | §3.2 の表 |
| `mode` | `manual` \| `automatic` | 必須 | 人が手で行った → manual。評価者が起動した script・cron が行った → automatic（Celeris 自身の自動処理は event 側なので書かない） |
| `intent` | enum | 必須 | `research`（研究内容のため）/ `safety`（安全・費用・外部送信の承認）/ `system`（道具・環境の都合）/ `protocol`（測定手順そのもの。介入に数えない）/ `monitoring`（状態の確認だけ） |
| `actor` | `{kind, id}` | 必須 | その op を行った主体。`kind`: §3.3 の 7 種（CLI の `--actor-kind` は必須で既定値は無い）。`id`: 仮名（メール・本名を書かない）、script 名、CoS・operator・MCP client の名前 |
| `decided_by` / `executed_by` / `approved_by` | `{kind, id}` \| null | 欄は必須（値は任意） | 判断した者・実行した者・承認した者。`actor` と同じ・分からないときは null。`kind`・`id` の規則は `actor` と同じ |
| `credential` | string | 必須 | 使った credential の種類: `admin_token` / `web` / `operator:<name>` / `mcp:<id>` / `cos_run` / `none` / `unknown`。token の値は書かない。CLI の既定は `unknown` |
| `request_ref` | string \| null | 欄は必須（op・wait で任意） | 論理要求 `<source_kind>:<source_key>`（受信箱の source の組。例 `question:<id>`・`notice:<key>`）。同じ問いの再通知・改訂は同じ値 |
| `request_revision` | string \| null | `request_ref` があれば必須 | 論理要求の revision（`cos_inbox_items.source_revision` と同じ文字列）。`request_ref` と両方か無しか |
| `policy_ref` | string \| null | 欄は必須（値は任意） | 適用した方針・既存承認への参照（例 `adr:2026-10-27-external-operator`、`approval:<id>`） |
| `start` / `end` | 時刻 \| null | `op_start` は `start`、`op_end` は `end` | §2.4 |
| `reason` | string | `resubmit`・`resume`・`system_*`・`manual_handoff`・`unclassified` では必須 | 1〜3 文。何が起きてなぜ必要だったか |
| `trigger` | enum | 必須 | `system_request`（システムが人を求めた: question・approval・prompt）/ `failure`（失敗・停止を見つけた）/ `scheduled`（手順で決めた確認）/ `self_initiated`（人の判断で） |
| `targets` | 配列 | 任意 | `{"kind": "artifact"\|"job"\|"task"\|"run"\|"session"\|"config", "ref": "...", "sha256": null}`。artifact は試行 workspace からの相対 path |
| `celeris_event_ref` | string \| null | B で対応 event があれば必須 | `event:<task_id>:<seq>`（`DecisionLogIntervention.source_event` と同じ形）。照合用 |
| `outcome` | `ok` \| `failed` \| `abandoned` \| null | `op_end` で必須 | — |
| `note` | string | 任意 | 自由記述（record_id の材料外） |

### 3.2 操作種別の定義表（A/B 同一）

| op_kind | 日本語 | 定義（記録する条件） | 既定の intent | intervention projection との対応 | 例（A / B） |
|---|---|---|---|---|---|
| `manual_transcription` | 手動転記 | 人が値・ファイル・ログを道具の間で写す（コピー・貼り付け・scp・手で表にする） | system | （なし。event に出ない） | A: クラスタの結果を手で手元へ写す / B: worker が読めない資料を人が workspace に置く |
| `status_check` | 状態確認 | 人が進捗・状態を見に行く（GUI・`squeue`/`qstat`・ログ・transcript）。何も変えない | monitoring | （なし） | A: 端末で Claude Code の出力を見る / B: GUI の task 画面を見る |
| `resubmit` | 再投入 | 失敗・停止した job・task・session を人が投入し直す | system（研究上の再試行なら research） | `Retried` → recovery | A: 失敗した `sbatch` を人が出し直す / B: failed task を Retry |
| `resume` | 再開 | 中断した作業を同じ状態から続けさせる（API キー更新後の再開、session の resume、continue） | system | （なし。API キー通知/再開は event 照合） | A: 上限で止まった Claude Code を `--resume` / B: blocked を解いて続行 |
| `research_direction` | 研究方針判断 | 研究の内容・方法・範囲を人が選ぶ（質問・decision への回答、方針の指示） | research | `Answered`・`DecisionAnswered`・`PlanGateDecided` → research_judgment | A: 次の実験条件を指示 / B: decisions に回答 |
| `safety_approval` | 安全承認 | 危険・費用・外部送信・credential 使用の可否を人が決める | safety | `ApprovalDecided` → unclassified（ここで safety に確定） | A: permission prompt で `rm`/外部送信を許可 / B: approval を承認 |
| `system_adjustment` | システム都合の調整 | 研究内容と無関係な道具・設定・権限・環境の手直し（失敗はまだ起きていない） | system | `Edited` → unclassified（調整なら確定） | A: CLAUDE.md・設定の修正 / B: task の編集・config の修正 |
| `system_recovery` | システム都合の復旧 | 障害・停止・壊れた状態から戻す（daemon/tunnel 再起動、詰まった process の kill、壊れた workspace の修復） | system | `IntegrationAnswered` → recovery | A: ssh master の張り直し / B: daemon の再起動 |
| `manual_handoff` | 人の代行実行 | 本来システムがやる作業工程を人が自分で実行する | research または system | → manual_handoff | A/B: 解析 script を人が回す |
| `protocol_step` | 測定手順 | 評価手順そのもの（試行開始・cutoff 宣言・grade 起動）。**介入に数えない** | protocol | （なし） | A/B: 試行を起動 |
| `unclassified` | 未分類 | 上のどれか決められない。推測で寄せない（理由必須） | 評価者が選ぶ | unclassified | — |

### 3.3 主体の種類（`actor.kind`。Celeris の actor_kind の唯一の語彙）

| kind | 意味 | 人の時間・人の介入に数えるか |
|---|---|---|
| `human` | 人（評価者・利用者）が自分で行った | `mode=manual` なら数える |
| `script` | 評価者が起動した script・cron | 数えない |
| `cos_project` | 案件の CoS（案件の固定スレッドの CoS run） | 数えない（主体別に別掲） |
| `cos_general` | 統括の CoS | 数えない（主体別に別掲） |
| `external_operator` | Celeris の外の operator（Claude Code 等の外部 session） | 数えない（主体別に別掲） |
| `mcp_client` | MCP client（ChatGPT RDC 等） | 数えない（主体別に別掲） |
| `unknown` | 主体を確かめられない。推測で `human` に寄せない | 数えない。`unknown_actor` に件数と区間を別掲（0 にしない） |

判定規則（A/B 共通）:
- 1 つの行為が 2 種に当たるときは、**その行為を必要にした原因** で選ぶ（障害が原因の再投入 → `resubmit` で `intent=system`、
  研究上の条件変更のための再投入 → `resubmit` で `intent=research`）。
- `status_check` は何かを変えたら別の op（変えた種別）を併記する。確認だけで終わったものも記録する（人の時間を使ったため）。
- 評価者自身の測定作業は `protocol_step`。人の介入の集計からは除き、別に件数だけ出す。
- CoS が人の代わりに答えた操作は `actor.kind=cos_project|cos_general`。人が CoS に判断を委ねた・CoS の案を人が承認したときは
  `decided_by`・`approved_by` に人を書く（`actor` は実際に操作した CoS）。

## 4. event 由来（記録しない）と評価者記録の区分表

| 量 | A（A0） | B（B0/B1/C1） | 生記録に書くか |
|---|---|---|---|
| 試行の identity・outcome・tokens | `TrialRecord` | `TrialRecord` | 書かない（導出） |
| 試行の壁時計 | `TrialRecord.wall_secs`（§2.4-6 で 0 を欠測へ） | 同左 | 書かない |
| 投入・開始時刻 | `LedgerRow.started_at`/`submitted_at` | 同左 ＋ `Created` event ts | 書かない |
| run の区間 | （run は無い。試行＝harness 1 回） | `runs.started_at`/`finished_at`、`WorkerStarted`/`WorkerFinished` | 書かない |
| ready_not_running | not_applicable | `Transitioned` で `ready` にいた区間のうち running run が無い時間 | 書かない |
| external_queue_wait | `job_observation`（scheduler 計上） | 主: `job_observation`、照合: `ClusterJobWaitStarted/Polled/Finished` | **書く**（A/B とも同じ計上値） |
| human_wait（システムが人を待つ） | `wait_start`/`wait_end`（transcript の ts があれば `time_basis=harness_transcript`） | `QuestionRaised` → `Answered`、`ApprovalRequested` → `ApprovalDecided`（`celerisctl campaign export` の `wait` 行。campaign の task 木の events から導出。決定の無い区間は `ended_at: null`）。`BrowserWaitOpened` と blocked 区間は未導出 | A だけ書く |
| 人の操作の種別・区間・理由 | `op_start`/`op_end` | `op_start`/`op_end`（event は `celeris_event_ref` で照合、加算しない） | **書く**（A/B 同一） |
| Celeris 内の自動 retry・repair・continuation | not_applicable（harness 内部の再試行は §6） | export の `run` 行の `is_retry`・`parent_run_id`・`attempt_no`（同じ task の自動再試行 run）。人が失敗 task を複製した `Retried` は介入（recovery）で、自動 retry に数えない | 書かない |
| 人の作業の attempt | `attempt` | `attempt` | **書く** |
| observation_cutoff | `cutoff` | `cutoff` | **書く** |
| 介入の件数（区分別） | 生記録から | 生記録から（projection は照合） | 生記録を正とする |
| 消費（tokens・費用） | `final_accounting` | `final_accounting` | 書かない |

## 5. 時間とやり直しの定義

端点（試行ごと）:
- `submitted`: `LedgerRow.submitted_at`（B は Celeris が task を受けた時刻）。A は `started_at` と同じ（投入と開始が同時）。
- `started`: `LedgerRow.started_at`。
- `ended`: 試行の終端（`TrialRecord` を書いた時点。ledger の `done`/`cancelled` 行）。値が無ければ null。
- `observation_cutoff`: 評価者の `cutoff` 行の `occurred_at`。無ければ `ended`（`started` + `TrialRecord.wall_secs`）。この終端より後に始まった op・attempt・区間は
  介入件数・人の時間・再作業のどれにも数えない（跨ぐ区間は終端で切る）。同じ試行 identity の試行開始前の記録（準備・再起動）は数える。

区間（いずれも `[start, end)`、端が null なら長さも null）:

| 区間 | 定義 | 出所 |
|---|---|---|
| 実行区間 | B: run の `started_at`〜`finished_at`（並列 run は重なるので elapsed と別に和を出す）。A: harness の開始〜終了 | runs / TrialRecord |
| ready_not_running | B: task または WU が `ready` で、running の run も外部待ちも無い区間。A: not_applicable | events |
| external_queue_wait | job の scheduler の submit〜start（実行待ち）。job の start〜end は「外部実行」で別に数える | `job_observation` |
| human_wait | システムが人の入力を求めてから人が応じるまで | B: events / A: `wait_*` |
| 人の操作時間 | `actor.kind=human` かつ `mode=manual` の `op_start.start`〜`op_end.end`（同じ `logical_op_id`・`attempt`・`op_kind`・`request_ref`・`request_revision`）。質問・承認が開いていた時間（human_wait）では代用しない | 生記録 |
| retry/recovery 区間 | 同じ論理作業の attempt n の失敗終了〜attempt n+1 の開始 | B 自動: runs.seq / 人: `attempt` |

介入の単位（論理要求）:
- 介入は `request_ref` で 1 件にまとめる（`request_ref` の無い op は 1 op = 1 件）。同じ要求の 2 件目以降の op（改訂・再通知・差戻し）は
  件数を増やさず、report の `intervention_items[].history` に時刻順に並べる。各要素の `event` は `initial`（最初）/ `revision`
  （revision が前と違う）/ `repeat`（同じ revision の再対応・差戻し）。介入の区分（op_kind）は最初の人の op の種別。
- `interventions`（人の介入）は `actor.kind=human` の manual の op を含む論理要求だけを数える。
- `interventions_by_actor_kind` は protocol を除く論理要求を、関わった主体の種類ごとに数える（1 件に複数の種類が関われば各種類に 1。
  合計は介入数を超え得る）。`actor.kind=unknown` の論理要求の件数と op 区間は `unknown_actor` に別掲する。
- 人以外の主体の op 件数は `excluded_ops.non_human`（主体の種類別）に残す。

やり直し:
- 同じ `logical_op_id` の `attempt` は 1 から連番。attempt 数 − 1 が「やり直し回数」。
- Celeris の自動 retry は export の `run` 行（`is_retry`）から導出し、`auto_retries` に別掲する（人の再作業に足さない）。人の作業・A の作業は評価者が付ける。
- 共通 filter: `protocol_step`・`intent=protocol`・`mode=automatic`・`actor.kind≠human` の op を含む論理作業は、介入件数・人の時間・再作業のどれにも数えない
  （protocol 上の trial-start `attempt=2` は科学的な redo ではない）。
- 観測の有無は項目ごと: その試行に op 記録（`op_start`/`op_end`。protocol・automatic を含む）が 1 件も無ければ介入件数・人の時間は欠測（`source=missing`、
  値 null）、attempt を持つ記録（`op_start`・`attempt`）が無ければ再作業は欠測。`cutoff`・`job_observation`・`wait_*` だけの試行を 0 にしない。
  arm 集計は欠測の試行を `human_time_missing_trials`・`rework_missing_trials` に数え、秒数・件数に 0 を足さない。
- 失敗・cancel・timeout の attempt も分母に残す（`runs_by_end` と同じ方針）。

## 6. 記録できない指標と主張の限定

- 評価者が気付かなかった・画面外で考えた時間は記録されない。人の時間は **下限** として報告し、「人の時間が X 減った」は
  記録された操作の範囲の主張に限る。人の時間は `actor.kind=human` の manual の区間だけで、質問が開いていた時間では代用しない。
- 主体の種類は記録者の申告で、認証 principal との照合はしない（`credential` は種類の記録）。主体が分からない操作は `unknown` として
  別掲し、人の操作にも 0 にも寄せない。
- 評価者が操作者を兼ねるため盲検ではなく、分類（op_kind・intent）は単一評価者の判断で、評価者間一致は測れない。
- A の harness 内部（Claude Code の tool 再試行・自動 compact・内部の待ち）は transcript を見ない限り見えない。
  A の ready_not_running は定義上存在しない（not_applicable）ため、B の待ち時間と同じ土俵で比べない。
- human_wait は B が event（秒精度）、A が評価者記録または transcript で、**測り方が arm で異なる**。比較表には `time_basis` を並べる。
- Celeris の intervention projection は `wall_ms` を持たず（欠測）、ApprovalDecided・Edited を区別しない。主体は event の `by` から
  `actor_kind`（human / cos_project / cos_general / external_operator / mcp_client / system / unknown）に分け、export の
  `human_interventions_by_kind` は `human` だけ、`interventions_by_actor_kind` は全主体を数える。`by` の無い（ADR
  2026-10-27-event-principals より前の）event は `unknown`（ADR 2026-10-27-decision-log-intervention-actors）。B の介入件数・区分は
  生記録を正とし、projection は件数の照合にだけ使う。campaign 識別の無い task の介入は projection に出ない。
- `TrialRecord.wall_secs` は整数秒で timeout で丸められる（超過分は分からない）。未実行・executor_error の 0 は欠測。
- host 間の時計のずれは補正しない。`clock_source=system_utc` 同士でも host が違う区間の長さは秒未満〜数秒の誤差を含み得る。
  scheduler 計上値と手元の時刻を混ぜた区間は `time_precision=approximate`。
- external_queue_wait はクラスタの混雑に依存し、システムの性能として帰属させない（別欄で報告）。
- 試行数は承認枠（12 試行以内）で小さく、統計的有意性は主張しない。差は記述として報告する。
- 研究の質・人の判断の質・認知負荷は測らない。

## 7. 成功契約と集計の範囲（checker・export・human-op で共通）

実装は `crates/campaign/src/success.rs`（ADR `agent-docs/adr/2026-10-23-campaign-success-contract.md`）。checker・export・`human-op join/report` は同じ関数で cell を判定し、report は再判定しない。

- cell = manifest の計画 (arm, case, rep)。**分母**は計画 cell 数で、未実行・未採点・欠測の cell も残す。join は runner の out dir（`campaign.json` がある dir）を `--events` に渡すと計画 cell をすべて行にする。
- **再試行の選択**: cell は `trial_no` 最小の記録で判定する。後の `trial_no` は `executor_trials` に数えるだけで成功に昇格させない。
- **executor status** と **validated success** は別欄。validated success = executor `passed` かつ結合検査（campaign hash・case・split・grader hash・成果物 hash・lock）を通った独立採点の `final_pass`。
  分類（`success_class`）: `validated_success` / `model_failure` / `grader_env_failure` / `grader_undetermined`（この 2 つは `needs_regrade`）/ `control_only` / `ungraded` / `executor_{failed,error,timeout,cancelled,blocked}` / `missing`。
- manifest・grades を渡さない入力では validated success は欠測（executor passed を成功に読み替えない）。
- 再採点は grades.jsonl を版として残し（`grade_out` の history）、再集計は同じ関数で行う。旧採点の結果を上書きしない。
- 消費の範囲（`export::COST_SCOPE`）: 主比較の tokens・費用は試行の run だけで、CoS の call と共有 overhead は含めず arm へ配分しない。
  定価換算と実支出は `final-accounting.json` で別欄のまま（加算しない）。欠測の試行は数え、0 で埋めない。
- 消費の欠測数の分母は「実行した試行」（`TrialRecord::ran`。checker の `ran_trials`）。実行されなかった cell（予算停止・依存 blocked・arm 予算切れで未投入）は
  report では `tokens_not_run_trials`・`cost_micro_usd_not_run_trials`（arm の `not_run_trials`）に数え、値は null のまま（欠測にも観測済み 0 にもしない）。
  記録の無い cell（`missing`）は欠測に数える。join の `success_class.ran` が cell の判定に使った試行の実行の有無。
- 照合: `python3 scripts/campaign/check-out-consistency.py --out <out>` が checker・export・human-op report の件数と分母を生記録からの再計算と突き合わせる（一致で exit 0）。

### 7.1 shared scope と訂正

- campaign 全体の共有作業（共有 env の修正など）は `human-op record --scope shared [--experiment-set main-ab|jev-bc]` で case/arm/rep を持たない行として 1 度だけ書く。
  report の `shared_overhead` に別掲し、arm へ配分しない。
- 訂正は追記専用: `human-op correct --file F (--corrects ID... | --logical-op L [--campaign C] [--arm A0]) (--retract | --set-field k=<JSON>...) --reason S`。
  元の行は書き換えず、`retracted: true` の対象は resolve で外れる。report の `corrections` と `report.md` の「補正履歴」に correction id・対象・理由が残る。
- 例: A/B 両方に記録した共有 env-lock 修正（71 秒）は、両方を `--retract` し、shared scope に 1 件書き直す。

### 7.2 CoS 消費の export

`celerisctl campaign export <campaign_id> --out <dir>` の `records.jsonl` の CoS usage 行は、
`thread_id`・`thread_kind`（general / project / legacy）・`project_id`・`model`・`session` を持つ。
`session` はその chat run の `session_row_id` に対応する `node_sessions.session_id`。
model は run の記録を優先し、無ければ同じ session の model。識別不明は null。
usage が無い終端 run も出力し、token・費用の欠測を null とする（観測値 0 と区別する）。

`summary.json.shared_overhead_by_kind` は `cos:general` と `cos:project` を分けて小計する。
過去の会話は `cos:legacy`、種類不明は `cos:unknown`。各値は観測数・欠測数を含む。
これは `shared_overhead` の内訳で、総小計に再加算しない。試行・arm への均等配分もしない。

campaign に未帰属の共有行も含むが、元の `campaign_id: null` を保つ。これらは全体の未帰属消費で、
選択した campaign の費用とはみなさない。campaign に明示帰属する共有行とは `campaign_id` で区別できる。
複数 campaign の export を結合するときは `dedup_key` で重複を除く。
