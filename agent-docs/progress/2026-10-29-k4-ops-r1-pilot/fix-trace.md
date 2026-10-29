---
title: ops-r1 B の trace と trial 別消費の読み取り専用 export
tasks: [01M62VP61K6XHRJW0H2YVX7N4D]
status: done
updated: 2026-10-29
---
# ops-r1 B の trace と trial 別消費の export

完了日: 2026-10-29

## 結論

棚卸しの不足 2 件を解消した。daemon（release 94f86928dc13）のコードは変えていない。

- g-b-trace-export: 解消。`celeris-campaign ops-r1-trace` が測定 daemon の DB の写しから 6 段（受付・起票・実行・
  成果確定・報告・cutoff）の時刻と参照を出す。欠けた段は null と reason。
- g-b-trial-attribution: 解消。`celeris-campaign ops-r1-attribution` が runs と chat_runs を 1 行ずつ trial_id・
  condition_id・role に帰属し、token 4 欄・wall・観測コストを出す。未帰属行は null と reason。監督 CoS の行は入れない。
- 使い方（DB の写しの作り方、読み取り専用であること、exit code）は `docs/ops/ops-r1-trace-attribution.md` §5。

## 変更（件名、基準は `git merge-base HEAD main`）

- `phase 1: ops-r1 trace・attribution の入力源と出力形を決める（verdict: feasible）`
- `phase 1: celeris-campaign ops-r1-trace（6 段の工程 trace の読み取り専用 export）と ops_r1_trace 試験を足す`
- `phase 1: celeris-campaign ops-r1-attribution（trial 別消費の読み取り専用 export）と ops_r1_attr 試験を足す`
- `phase 1: ops-r1 attr の進捗を書く`
- `phase 1: ops-r1 fix-trace の close（実行例と検査結果）` — 文書に実行例を足し、この進捗をまとめた

## 証拠（close、提出時の HEAD で実行）

- `cargo test -p celeris --test ops_r1_trace --test ops_r1_attr`: exit 0、ops_r1_attr 5 passed・ops_r1_trace 5 passed
- `bash scripts/dev/test-affected.sh`（基準 `$CELERIS_WU_BASE`）: exit 0、「試験対象の変更なし（docs のみ等）」
- `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh`（task 全体の差分）: exit 0、
  nextest 2824 passed / 9 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- 文書検査 3 本: 下の「文書検査」節
- `git diff --name-only $(git merge-base HEAD main) | grep -v '^crates/campaign/\|^crates/celeris/tests/\|^scripts/campaign/\|^docs/ops/\|^agent-docs/'`: 0 行

## design（WU design、2026-10-29 完了）

verdict は feasible。daemon（release 94f86928dc13、schema 版 102）のコード変更は要らない。

- 書いたもの: `docs/ops/ops-r1-trace-attribution.md`（6 段の取り元・reason の語彙・attribution の行の形と帰属規則・
  byte 一致の規則）と ADR `agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md`。crates/ は変えていない。
- 調べた schema: `chat_threads`・`chat_messages`・`chat_runs`（0050・0061・0092・0102）、`cos_operations`（0050）、
  `events`（0002）、`tasks`（`parent_id`・`root_id`）、`runs`（0026・0071）、`root_done_summaries`（0080）、
  `decision_log_usage`（0082、使わない）。CoS の `task.create` は `cos_operations` に `target_kind='task'`・`target_id`=新 task、
  監査 event は新 task の stream に入る（`task-core/src/chat/operations.rs`）。報告は root done のとき daemon が
  案件 CoS thread に積む依頼への CoS の返答（`task-dispatch/src/dispatcher/cos_chat/root_done.rs`）。
- DB に無いもの: condition_id と cutoff。どちらも引数で必須にする。
- 決めたこと: subcommand 名は `ops-r1-trace`・`ops-r1-attribution`（既存 `ops-r1-intake` に揃える）。
  受付 message は `--intake-record` の `message_id`、無ければ `client_message_id='ops-r1-intake:<trial_id>'`。

## trace（WU trace、2026-10-29 完了）

`celeris-campaign ops-r1-trace --db <写し> --thread <id> --trial-id <id> --cutoff <RFC3339> [--intake-record <JSON>] --out <path>`
を足した。daemon 側の crate は変えていない。

- 実装: `crates/campaign/src/ops_r1_trace.rs`（main.rs は `ops-r1-trace` の分岐だけ。exit 0 = 出力、1 = 読み取りの失敗、
  2 = 入力の誤り）。rusqlite を celeris-campaign の通常依存へ移した（Cargo.lock は変化なし）。
- DB は `file:<path>?mode=ro&immutable=1` と `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI` で開く。`-wal` があれば exit 2、
  schema 版 102 以外は exit 2、`--thread` が `project` 以外か存在しなければ exit 2。
- 6 段は設計 §2 どおり。`--intake-record` は `message_id` を読み、`thread_id`・`trial_id` が書いてあれば引数と照合する
  （ops-r1-intake の記録の形はこの branch に未取り込みのため、この 3 欄だけを見る）。
- 試験: `crates/celeris/tests/ops_r1_trace.rs`（fixture は `crates/celeris/tests/ops_r1/trace_fixture.rs`。actor_trace/support.rs は
  変えていない）。本物の store で案件・案件 CoS thread・`task.create` の CoS operation（task と監査 event を同じ transaction）・
  子 task・review 判定・done 遷移を書き、chat message・chat run・run・root_done_summaries の行を足し、`VACUUM INTO` で単一
  ファイルの写しを作る。5 件: 6 段が全部出る／報告 message が欠けると `report` が null と `report_run_not_completed`／
  cutoff より後は無いものとして扱い上流の欠けが伝わる／2 回の出力が byte 一致し写しの sha256 が変わらず `-wal`・`-shm` を作らない／
  不正な thread・cutoff・`-wal` 付きの DB は exit 2。

### 検査（提出時の HEAD で実行、基準は `$CELERIS_WU_BASE`）

- `cargo test -p celeris --test ops_r1_trace`: 5 passed
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 2817 passed、9 skipped）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/ | grep -v '^crates/campaign/\|^crates/celeris/tests/'`: 空

## attr（WU attr、2026-10-29 完了）

`celeris-campaign ops-r1-attribution --db <写し> --thread <id> --trial-id <id> --condition-id <id> --cutoff <RFC3339>
[--intake-record <JSON>] --out <path>` を足した。daemon 側の crate は変えていない。

- 実装: `crates/campaign/src/ops_r1_attr.rs`。DB の開き方・受付・起票・子孫の解決は `ops_r1_trace` の関数
  （`open_copy`・`intake_stage`・`created_stage`・`trial_tasks`）を共有する。main.rs は `ops-r1-attribution` の分岐だけ。
- 設計に足した細部: chat run の `started_at` が NULL の行は `time_reason: "not_started"`、`usage_json` が JSON でない行は
  `usage_reason: "usage_invalid"`。`role` が決まらない帰属行は `totals` の key を `other:<role_raw>` にする。
  `role_raw` は常に元の値（runs.role か thread の kind）を出す。
- 試験: `crates/celeris/tests/ops_r1_attr.rs`（fixture は `ops_r1/trace_fixture.rs` の写しに試行外の run・統括 CoS の
  参照あり/なしの turn・受付前の turn を足す `ops_r1/attr_fixture.rs`）。5 件: role ごとの行と合計、試行外 run の除外、
  未帰属の null と reason（`no_trial_reference`・`before_intake`・`trial_roots_unknown`）、2 回の出力の byte 一致と
  写しの sha256 不変、入力の誤りで exit 2。
- 検査（提出時の HEAD で実行、基準は `$CELERIS_WU_BASE`）:
  - `cargo test -p celeris --test ops_r1_trace --test ops_r1_attr`: exit 0、5 + 5 passed
  - `bash scripts/dev/test-affected.sh`: exit 0、2824 passed / 9 skipped
  - `cargo clippy --workspace -- -D warnings`: exit 0
  - `cargo fmt --all -- --check`: exit 0
  - `git diff --name-only $CELERIS_WU_BASE -- 'crates/task-*' crates/celeris/src web`: 0 行

design の検査（WU design の提出時）: `sh scripts/dev/check-doc-links.sh docs/ops/ops-r1-trace-attribution.md
agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md` → ok。

## 文書検査（close、提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh docs/ops/ops-r1-trace-attribution.md agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md agent-docs/progress/2026-10-29-k4-ops-r1-pilot/fix-trace.md`: exit 0、`check-doc-links: ok`
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0、`check-doc-layout: ok`
- `sh scripts/dev/progress-index.sh --check`: exit 0、`progress-index --check: ok`

## 未解決事項

- ADR 2026-10-29-ops-r1-cos-intake-record（task 01M62TJ5V2B65Y0C804HHQ4E3Q）はこの branch に未取り込み。取り込まれたら
  ADR 2026-10-29-ops-r1-trace-attribution-export の関係欄をリンクにする。`--intake-record` は `message_id`・`thread_id`・
  `trial_id` の 3 欄だけを見ており、ops-r1-intake の記録の実物との突き合わせは未確認。
- 統括 CoS が読むだけで操作しない turn は試行に結べず `no_trial_reference` で未帰属に残る。
- 出力に chat run の model は入れていない（`resolved_config_json` の key は未確認）。
- 実の測定 daemon の DB の写しではまだ実行していない。試験は fixture の DB だけで確かめた。

## 提案

- runbook（K4 pilot の prep）に §5 の手順（`.backup` で写し → 2 つの subcommand → 写しの sha256 照合）を入れ、
  試行ごとに `db-copy.sha256`・`trace.json`・`attribution.json` と標準出力の sha256 を記録に残す。
- celeris-campaign の binary は daemon と別に pin する（daemon は 94f86928dc13 のまま）。
