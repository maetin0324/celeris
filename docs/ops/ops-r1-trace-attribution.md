---
tasks: [01M62VP61K6XHRJW0H2YVX7N4D]
---
# ops-r1 B の trace と trial 別消費の export（入力源と出力形）

測定 daemon（本番昇格済み release 94f86928dc13、schema 版 102）の DB の写しだけから、B の 1 試行の
工程 trace と trial 別の消費を出す。道具は `celeris-campaign` の 2 つの subcommand で、daemon のコードは変えない。
判断の理由は ADR `agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md`。

verdict: feasible

6 段・消費の 4 欄・wall・観測コストはどれも 94f86928dc13 の DB に既にある。daemon 側に足す欄は無い。
DB に無いのは condition_id と cutoff の 2 つだけで、どちらも引数で渡す。

## 1. 入力と DB の開き方

```sh
celeris-campaign ops-r1-trace --db <DB の写し> --thread <案件 CoS thread id> --trial-id <id> \
  --cutoff <RFC3339 UTC> [--intake-record <ops-r1-intake/1 の JSON>] --out <trace.json>
celeris-campaign ops-r1-attribution --db <DB の写し> --thread <id> --trial-id <id> \
  --condition-id <id> --cutoff <RFC3339 UTC> [--intake-record <JSON>] --out <attribution.json>
```

- subcommand 名は既存の `ops-r1-intake` に揃えて `ops-r1-trace`・`ops-r1-attribution` とする。
- `--db` は `sqlite3 <src> ".backup <dst>"` で作った単一ファイルの写し。`<db>-wal` があれば exit 2（写しの bytes が
  1 つに決まらないため）。開き方は `file:<path>?mode=ro&immutable=1` の URI と `SQLITE_OPEN_READ_ONLY`。
  `SqliteStore::open` は migration を走らせるので使わない。rusqlite で直接読む。
- `schema_migrations` の最大版が 102 でなければ exit 2（読む表・欄は版 102 で確かめた）。
- `--thread` は `chat_threads.kind='project'` の thread に限る。他の kind・存在しない id は exit 2。
  案件（`project_id`）はこの thread から決める。
- `--cutoff` は必須。測定の打ち切り時刻で、runbook が固定する。報告 message の時刻から決めない（報告が欠けた試行で
  cutoff が決まらなくなるのと、実行時の時計を入れると byte 一致が崩れるため）。
- `--condition-id` は DB に無いので引数で渡す（attribution だけ必須。trace は取らない）。
- exit: 0 = 出力した（欠けた段・未帰属行があっても 0）、2 = 入力の誤り（上の各条件、`--intake-record` と
  `--thread`・`--trial-id` の食い違い）、1 = 読み取りの失敗（SQL・I/O）。

## 2. trace の 6 段

出力は `{"schema":"ops-r1-trace/1", trial_id, thread_id, project_id, db:{schema_version, sha256}, cutoff, stages:[…]}`。
`stages` は下の順の 6 要素で、各要素は `{stage, at, ref, reason}`。段が取れたら `reason` は null、
欠けたら `at` と `ref` が null で `reason` に下の語彙を入れる。cutoff より後の行・event は無いものとして扱う。
前の段が欠けて決まらない段は `reason: "upstream_missing:<段名>"` にする。

| 段 | 取り元 | at | ref | 欠けたときの reason |
|---|---|---|---|---|
| `intake` 受付 message | `chat_messages`。`--intake-record` があればその `message_id`、無ければ `thread_id=--thread` かつ `client_message_id='ops-r1-intake:<trial_id>'` | message の `created_at` | `message_id`・`seq`・`client_message_id`・`author`（`metadata_json.$.author`）・取り込んだ CoS run（`chat_runs.input_message_id` が一致する `run_id`、無ければ null） | `intake_message_not_found`・`intake_not_human`（`role<>'user'` か author が `human:` で始まらない）・`intake_after_cutoff` |
| `created` 起票 | `cos_operations`（`thread_id=--thread`・`action='task.create'`・`state='applied'`・`created_at` が受付以後 cutoff 以前）と、その監査 event（`events` の `type='cos_operation'` かつ `operation_id` 一致。task 対象の監査は起票した task 自身の stream に入る） | 最初の起票の `created_at` | `operation_id`・`cos_run_id`（`cos_operations.run_id`）・`audit_event_id`（`events.id`）・`root_task_id`（`target_id`）・`created_by`（`Created` event の `created_by`、`cos:<thread_id>` のはず）・`extra_root_task_ids`（同じ窓の 2 件目以降の起票、`created_at, id` 順） | `no_task_create_operation`・`task_create_not_applied`（rejected 等しか無い） |
| `executed` task と run | root 群とその子孫（`tasks.parent_id` の再帰）の `tasks` と `runs` | 子孫の run の最小 `started_at`（`ref.until` に最大 `finished_at`） | `task_ids`（昇順）・`runs`（`run_id`・`task_id`・`role`・`status`・`started_at`・`finished_at`、`started_at, run_id` 順） | `no_runs` |
| `outcome` 成果確定 | root task の `events`: cutoff 以前で最後の `type='transitioned'` かつ `to` が `done`・`failed`・`cancelled`。根拠として同じ task の `reviewer_passed`・`required_checks_evaluated`・`review_verdict` | その event の `ts` | `task_id`・`event_id`・`seq`・`to`・`reason`・`reviewer_passed`（`events.id` の配列）・`required_checks`（同）・`review_verdicts`（件数） | `not_terminal_by_cutoff` |
| `report` 報告 message | `root_done_summaries`（`task_id`=root、`state`）→ `input_message_id` を持つ `chat_runs` → その `output_message_id` の `chat_messages`（`role='assistant'`） | 報告 message の `created_at` | `thread_id`・`input_message_id`・`chat_run_id`・`message_id`・`root_done_state` | `root_not_done`（成果確定が `done` でない）・`report_not_queued`（行が無い）・`report_fallback`（`state='fallback'`。CoS の文が無く daemon の定型文だけ）・`report_run_not_completed`・`report_after_cutoff` |
| `cutoff` | 引数 `--cutoff` | 引数の時刻 | `source: "argument"`・`report_before_cutoff`（報告が取れたら true、欠けたら null） | 無し（引数が必須） |

報告は root task が `done` になったときに daemon が案件の CoS thread へ積む依頼（ADR 2026-10-15-root-done-cos-summary）
への CoS の返答とする。root が `failed`・`cancelled` で終わった試行には報告の段が無い（`root_not_done`）。

## 3. attribution の行の形

出力は `{"schema":"ops-r1-attribution/1", trial_id, condition_id, thread_id, project_id, cutoff, window:{from, to},
root_task_ids, db:{schema_version, sha256}, rows:[…], totals, unattributed}`。`window.from` は受付の時刻、
`window.to` は cutoff。

候補の行は DB の `runs` 全行と `chat_runs` 全行のうち `started_at` が cutoff 以前のもの（測定 daemon は試行専用の
state なので、取りこぼしを見えるように全部出す）。1 run が 1 行。

| 欄 | 中身 |
|---|---|
| `source` | `runs` か `chat_runs` |
| `run_id`・`task_id`・`thread_id` | 元の行の値（持たない側は null） |
| `trial_id`・`condition_id` | 帰属したときだけ引数の値、未帰属なら null |
| `role` | `runs.role` の `planner`・`worker`・`reviewer`。chat run は thread の kind で `project` → `cos_project`、`general`・`review` → `cos_general`。それ以外は null で `role_raw` に元の値 |
| `input_tokens`・`output_tokens`・`cache_read_tokens`・`cache_write_tokens` | `usage_json` の `input_tokens`・`output_tokens`・`cache_read_tokens`・`cache_creation_tokens` |
| `wall_ms` | `finished_at - started_at`（終わっていなければ null） |
| `cost_usd`・`cost_basis` | `usage_json.cost_usd`（daemon の単価表による推定）と `"pricing_estimate"`。現金の支払い額ではない |
| `usage_reason` | `usage_json` が無ければ `usage_missing`（4 欄と cost は null）、あれば null |
| `reason` | 帰属したら null、未帰属なら下の語彙 |

帰属の規則（上から順に最初に当たったもの）:

1. 受付か起票が欠けて root が決まらなければ、全行が未帰属で `reason: "trial_roots_unknown"`。
2. `started_at` が受付より前 → `before_intake`。
3. `runs` の行: `task_id` が root 群とその子孫にあれば帰属。無ければ `task_not_in_trial_tree`（cron・KB 整理などの
   support task もここに入る）。
4. `chat_runs` の行で thread が `--thread`: 帰属（案件 CoS の turn）。
5. thread が他の `project` thread → `other_project_thread`。
6. thread が `general`・`review`（統括 CoS）: 次のどれかの参照があるときだけ帰属する。
   (a) その `run_id` の `cos_operations` が試行の task（`target_kind='task'` で `target_id` が試行の task、
   または `result_json.$.task_id` が試行の task）か試行の案件（`target_kind='project'`・`target_id`=案件）を対象にしている。
   (b) その run の `input_message_id` が試行の root の `root_done_summaries.input_message_id` と一致する。
   参照が無ければ `no_trial_reference`。読むだけの turn は試行に結べないので、推測で入れず未帰属として出す。
7. それ以外の thread kind → `other_thread`。

`totals` は帰属行だけを `role` ごとに合計する（4 欄・wall・cost。null は合計に入れず、`null_count` を別に数える）。
`unattributed` は未帰属行を `reason` ごとに数え、同じ 4 欄の合計も出す。帰属行と未帰属行の合計が DB の候補行の全量になる。

監督 CoS（外部 operator の Claude Code、本番 daemon の CoS）の行は入れない。その消費は測定 daemon の DB に無く、
別の DB や transcript を結ぶと「同じ DB から 2 回出して byte 一致」が成り立たなくなる。B の条件の外の支援として
operator_usage・human_ops の記録（外部 AI による支援）で別に数える。

## 4. byte 一致のための規則

- 時刻はすべて parse して比べる（RFC3339。DB には秒止めと小数付きが混ざる）。出力は UTC の
  `YYYY-MM-DDTHH:MM:SS.mmmZ`（小数 3 桁に切り捨て）。parse できない値は時刻を null にし、
  段なら `reason: "bad_timestamp"`、行なら `usage_reason` とは別に `time_reason: "bad_timestamp"` を付ける。
- 並び: trace の段は固定順。配列は上の表の順（task id は昇順、run は `started_at, run_id`、event は `events.id`）。
  attribution の `rows` は `(started_at, source, run_id)`、時刻が null の行は末尾で `(source, run_id)`。
  `totals`・`unattributed` の key は BTreeMap の辞書順。
- 実行時刻・host 名・`--db` の絶対 path は出さない。DB の同定は写しの sha256。
- JSON は struct の欄順で整形し（`serde_json::to_string_pretty`）、末尾に改行 1 つ。cost の合計は上の並び順で足す。
- `--out` は毎回上書きし、出力の sha256 を標準出力に 1 行出す。

## 5. 試験（次の葉）

試験名は `ops_r1_trace`・`ops_r1_attr` で始め、`crates/celeris/tests/actor_trace/support.rs` の Harness に
helper を足して flow1 相当の fixture を作る（受付 message・`task.create` の CoS operation・子 task と planner/worker/reviewer
の run・done への遷移と reviewer_passed・root_done の報告 message・統括 CoS の参照あり/なしの turn）。
LLM は呼ばない。同じ DB の写しから 2 回出して bytes が一致することを確かめる。
