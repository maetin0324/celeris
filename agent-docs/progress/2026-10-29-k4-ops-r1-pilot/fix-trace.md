---
title: ops-r1 B の trace と trial 別消費の読み取り専用 export
tasks: [01M62VP61K6XHRJW0H2YVX7N4D]
status: running
updated: 2026-10-29
---
# ops-r1 B の trace と trial 別消費の export

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

## 検査（提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh docs/ops/ops-r1-trace-attribution.md agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md`: ok
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/`: 空

## 未解決事項

- ADR 2026-10-29-ops-r1-cos-intake-record（task 01M62TJ5V2B65Y0C804HHQ4E3Q）はこの branch に未取り込み。取り込まれたら ADR の関係欄をリンクにする。
- 統括 CoS が読むだけで操作しない turn は試行に結べず `no_trial_reference` で未帰属に残る。
- `chat_runs` の model を出すかは未確認（`resolved_config_json` の key を実装葉で確かめる）。今の行の形には model を入れていない。

## 提案

- 実装の葉（trace・attr）は試験を `crates/celeris/tests/ops_r1_trace*.rs`・`ops_r1_attr*.rs` に置き、
  `actor_trace/support.rs` に helper を足して fixture を作る。
