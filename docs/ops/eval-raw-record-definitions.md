# 多軸評価の生記録（human-ops JSONL）の定義: identity・時計・人の操作種別・時間とやり直し・限定事項

---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
---

根拠: ADR `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md`。この文書は **測定開始前に固定する定義** で、
評価者は A（A0）と B（B0）、および Jev 副比較（B1/C1）で同じ定義を使う。実装（型・CLI・結合）はこの定義に従う。
形式を変えるときは `schema` の版を上げ、ADR に付記する。

## 1. ファイルと行

- 場所: campaign の out ディレクトリ（`trials.jsonl`・`ledger.jsonl` と同じ）に `human-ops.jsonl`。
- 1 行 = 1 record（UTF-8 JSON、改行区切り）。`schema = "celeris.eval-human-op/1"`。
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
| `schema` | string | 必須 | — | `celeris.eval-human-op/1` |
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
  `trial_no`, `logical_op_id`, `attempt`, `op_kind`, `wait_kind`, `job_ref`, `corrects`（無い欄は null として入れる）。
  `op_start`/`op_end` と `wait_start`/`wait_end` は `record_type` が違うので別 id。同じ論理作業・同じ attempt の同じ種別の
  開始は 1 件しか存在しない。
- 材料に入れない: `recorded_at`, `clock_source`, `host`, `note`, 時刻の値そのもの。→ 同じ事実を再送・再開・二重実行で
  書き直しても id は変わらない。
- 追記時: 同じ id が既にあり、本体（材料外の欄で `recorded_at`/`clock_source`/`host` を除いたもの）が同じなら **no-op**
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
| `actor` | `{kind, id}` | 必須 | `kind`: `human` / `script`。`id`: 評価者の仮名（メール・本名を書かない）か script 名 |
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
| `research_direction` | 研究方針判断 | 研究の内容・方法・範囲を人が選ぶ（質問・decision への回答、方針の指示） | research | `Answered` → research_judgment | A: 次の実験条件を指示 / B: decisions に回答 |
| `safety_approval` | 安全承認 | 危険・費用・外部送信・credential 使用の可否を人が決める | safety | `ApprovalDecided` → unclassified（ここで safety に確定） | A: permission prompt で `rm`/外部送信を許可 / B: approval を承認 |
| `system_adjustment` | システム都合の調整 | 研究内容と無関係な道具・設定・権限・環境の手直し（失敗はまだ起きていない） | system | `Edited(by=human)` → unclassified（調整なら確定） | A: CLAUDE.md・設定の修正 / B: task の編集・config の修正 |
| `system_recovery` | システム都合の復旧 | 障害・停止・壊れた状態から戻す（daemon/tunnel 再起動、詰まった process の kill、壊れた workspace の修復） | system | → recovery | A: ssh master の張り直し / B: daemon の再起動 |
| `manual_handoff` | 人の代行実行 | 本来システムがやる作業工程を人が自分で実行する | research または system | → manual_handoff | A/B: 解析 script を人が回す |
| `protocol_step` | 測定手順 | 評価手順そのもの（試行開始・cutoff 宣言・grade 起動）。**介入に数えない** | protocol | （なし） | A/B: 試行を起動 |
| `unclassified` | 未分類 | 上のどれか決められない。推測で寄せない（理由必須） | 評価者が選ぶ | unclassified | — |

判定規則（A/B 共通）:
- 1 つの行為が 2 種に当たるときは、**その行為を必要にした原因** で選ぶ（障害が原因の再投入 → `resubmit` で `intent=system`、
  研究上の条件変更のための再投入 → `resubmit` で `intent=research`）。
- `status_check` は何かを変えたら別の op（変えた種別）を併記する。確認だけで終わったものも記録する（人の時間を使ったため）。
- 評価者自身の測定作業は `protocol_step`。人の介入の集計からは除き、別に件数だけ出す。

## 4. event 由来（記録しない）と評価者記録の区分表

| 量 | A（A0） | B（B0/B1/C1） | 生記録に書くか |
|---|---|---|---|
| 試行の identity・outcome・tokens | `TrialRecord` | `TrialRecord` | 書かない（導出） |
| 試行の壁時計 | `TrialRecord.wall_secs`（§2.4-6 で 0 を欠測へ） | 同左 | 書かない |
| 投入・開始時刻 | `LedgerRow.started_at`/`submitted_at` | 同左 ＋ `Created` event ts | 書かない |
| run の区間 | （run は無い。試行＝harness 1 回） | `runs.started_at`/`finished_at`、`WorkerStarted`/`WorkerFinished` | 書かない |
| ready_not_running | not_applicable | `Transitioned` で `ready` にいた区間のうち running run が無い時間 | 書かない |
| external_queue_wait | `job_observation`（scheduler 計上） | 主: `job_observation`、照合: `ClusterJobWaitStarted/Polled/Finished` | **書く**（A/B とも同じ計上値） |
| human_wait（システムが人を待つ） | `wait_start`/`wait_end`（transcript の ts があれば `time_basis=harness_transcript`） | `QuestionRaised`/`ApprovalRequested`/`BrowserWaitOpened` → `Answered`/`ApprovalDecided`/`BrowserWaitResolved`、`Transitioned` blocked 区間 | A だけ書く |
| 人の操作の種別・区間・理由 | `op_start`/`op_end` | `op_start`/`op_end`（event は `celeris_event_ref` で照合、加算しない） | **書く**（A/B 同一） |
| Celeris 内の自動 retry・repair・continuation | not_applicable（harness 内部の再試行は §6） | `runs.seq`、`RepairScheduled`、`IntegrationRepair*`、`Retried` | 書かない |
| 人の作業の attempt | `attempt` | `attempt` | **書く** |
| observation_cutoff | `cutoff` | `cutoff` | **書く** |
| 介入の件数（区分別） | 生記録から | 生記録から（projection は照合） | 生記録を正とする |
| 消費（tokens・費用） | `final_accounting` | `final_accounting` | 書かない |

## 5. 時間とやり直しの定義

端点（試行ごと）:
- `submitted`: `LedgerRow.submitted_at`（B は Celeris が task を受けた時刻）。A は `started_at` と同じ（投入と開始が同時）。
- `started`: `LedgerRow.started_at`。
- `ended`: 試行の終端（`TrialRecord` を書いた時点。ledger の `done`/`cancelled` 行）。値が無ければ null。
- `observation_cutoff`: 評価者の `cutoff` 行の `occurred_at`。無ければ `ended`。cutoff 後の出来事は数えない。

区間（いずれも `[start, end)`、端が null なら長さも null）:

| 区間 | 定義 | 出所 |
|---|---|---|
| 実行区間 | B: run の `started_at`〜`finished_at`（並列 run は重なるので elapsed と別に和を出す）。A: harness の開始〜終了 | runs / TrialRecord |
| ready_not_running | B: task または WU が `ready` で、running の run も外部待ちも無い区間。A: not_applicable | events |
| external_queue_wait | job の scheduler の submit〜start（実行待ち）。job の start〜end は「外部実行」で別に数える | `job_observation` |
| human_wait | システムが人の入力を求めてから人が応じるまで | B: events / A: `wait_*` |
| 人の操作時間 | `op_start.start`〜`op_end.end`（同じ `logical_op_id`・`attempt`・`op_kind`） | 生記録 |
| retry/recovery 区間 | 同じ論理作業の attempt n の失敗終了〜attempt n+1 の開始 | B 自動: runs.seq / 人: `attempt` |

やり直し:
- 同じ `logical_op_id` の `attempt` は 1 から連番。attempt 数 − 1 が「やり直し回数」。
- Celeris の run は `logical_op_id = <task_id>`、`attempt = runs.seq`（導出、書かない）。人の作業・A の作業は評価者が付ける。
- 失敗・cancel・timeout の attempt も分母に残す（`runs_by_end` と同じ方針）。

## 6. 記録できない指標と主張の限定

- 評価者が気付かなかった・画面外で考えた時間は記録されない。人の時間は **下限** として報告し、「人の時間が X 減った」は
  記録された操作の範囲の主張に限る。
- 評価者が操作者を兼ねるため盲検ではなく、分類（op_kind・intent）は単一評価者の判断で、評価者間一致は測れない。
- A の harness 内部（Claude Code の tool 再試行・自動 compact・内部の待ち）は transcript を見ない限り見えない。
  A の ready_not_running は定義上存在しない（not_applicable）ため、B の待ち時間と同じ土俵で比べない。
- human_wait は B が event（秒精度）、A が評価者記録または transcript で、**測り方が arm で異なる**。比較表には `time_basis` を並べる。
- Celeris の intervention projection は `wall_ms` を持たず、ApprovalDecided・Edited を区別しない。B の介入件数・区分は生記録を正とし、
  projection は件数の照合にだけ使う。campaign 識別の無い task の介入は projection に出ない。
- `TrialRecord.wall_secs` は整数秒で timeout で丸められる（超過分は分からない）。未実行・executor_error の 0 は欠測。
- host 間の時計のずれは補正しない。`clock_source=system_utc` 同士でも host が違う区間の長さは秒未満〜数秒の誤差を含み得る。
  scheduler 計上値と手元の時刻を混ぜた区間は `time_precision=approximate`。
- external_queue_wait はクラスタの混雑に依存し、システムの性能として帰属させない（別欄で報告）。
- 試行数は承認枠（12 試行以内）で小さく、統計的有意性は主張しない。差は記述として報告する。
- 研究の質・人の判断の質・認知負荷は測らない。
