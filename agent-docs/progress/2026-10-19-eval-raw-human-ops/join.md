---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
status: done
completed: 2026-10-19
---
# 多軸評価 2a/P0: event 結合の読み出し（WU join）

## 実装
- `celeris-campaign human-op join --file <human-ops.jsonl> --events <export dir | out dir | file>`（`crates/campaign/src/human_ops_join.rs`、cli の dispatch に `join` を追加）。
- events 入力: export の `records.jsonl`（run/outcome/intervention）、campaign out の `trials.jsonl`（TrialRecord.wall_secs）・`ledger.jsonl`（started_at/submitted_at/task/run）。どれも無ければ exit 1。
- 出力は JSONL。identity（manifest_sha256, case, arm(A/B/C), rep）ごとに 1 行。各欄は `{value, source}`、source は `event` / `evaluator` / `missing`。`missing_fields`・`missing_count` を付ける。
- Celeris event から取れる区間（wall_secs, run_wall_ms, started/submitted, run_ids, 介入の件数・区分）は再記録せず導出。人の操作・attempt・待ち・job 観測は評価者記録。
- 欠測は 0 にしない: op_end が無い操作は `human_op_secs.missing` に数え、blocked/未実行/error の wall_secs=0 は missing。A 腕は export の run 行が無いので `event_interventions` 等が missing のまま。
- 冪等: events は `dedup_key` / 台帳の鍵 / trial_no で潰し、評価者記録は record_id で潰す。再 export・再実行で出力は byte 一致。
- task_id が試行の run に結べない介入は `unjoined_event` 行、identity の無い評価者記録は `unjoined_evaluator` 行に出所つきで残す（捨てない）。集計・表は 2b。

## 証拠
- `cargo test -p celeris-campaign --test human_ops_join`: 2 passed（mock 実行 A/B/C の trial と同 identity で結合、二重 events・command 再実行で出力一致、A 腕の missing_count>=4）。
- `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0。
- `bash scripts/dev/test-parallel.sh`: nextest passed 5832 failed 0、doctest ok、exit 0。

## 未解決・提案
- B0 と B1 は同じ ConfigId::B なので、events 側は campaign（manifest）が別であることで区別する。同じ manifest に両方を入れる運用は結合が曖昧になる。
- 結合は export の intervention を task_id 経由でしか試行に結べない。campaign 識別の無い task の介入は `unjoined_event` になる（SPEC の限定どおり）。
