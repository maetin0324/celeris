---
title: IEEE recovery scheduler — 正式入口に PBS backend と normal/A2、別 process controller 停止・同一 job 再開（harness-proc）
tasks: [01M5T2P8MAW1564BFKSXXV33YG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 正式入口に PBS backend と normal/A2、別 process controller 停止・同一 job 再開（harness-proc）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。ADR 付記 I14 (1)(2)(3) を実装した。

## やったこと

- `crates/campaign/src/dagbench/pbs/file_pbs.rs`（新規）: `FilePbs`（`PbsTransport` + `entry::Site`）。state dir の
  `jobs.jsonl`・`calls.jsonl`（呼んだ pid 付き）・`outputs/<job>/out.bin`・`faults.json`（`FilePbsFaults`）、排他は `pbs.lock`
  （`File::lock`）、in-memory 状態なし。job ID は `<4300+n>.sirius`、解放までは `R`、以後 `F`/`Exit_status = 0`。
  `FileClock`（`clock.json`、`sleep_until` は待たずに跳ぶ）。`counts()`・`jobs()`・`calls()` は試験の assert 用。
- `crates/campaign/src/dagbench/pbs/proc.rs`（新規）: 子 `child_trial`（`RecordingTransport<FilePbs>` が原票を
  `replies-<inc>.jsonl` に行ごと sync、境界で `PBS_WAIT_SAVED {…}` を出して stdin の `CONTINUE` を待つ）と
  親 `ProcTrial::run`（`Supervisor`/`OwnedProc`、`/proc/<pid>/stat` の starttime、独立接続での保存の読み直し、
  intent 台帳と `replies-1.jsonl` の job 照合、`durable-wait.json` + sha256、normal は解放 → `CONTINUE`、A2 は starttime 再照合 →
  所有 `Child` の SIGKILL（exit_signal 9 を確認）→ 解放 → 親の `FilePbs` で `F` まで照会して raw へ → incarnation 2 → 回収、
  重複投入（intent 行の増加）・重複消費・job 不一致の検出、独立 grade、`a2.json`）。barrier は trial dir の `process-events.jsonl`。
- `entry.rs`: `BackendFlag` に serde と `from_env_value`、`Condition::parse`、子用の `child_gated`（`Gate` の境界に `announce` を足す）。
- `raw.rs`: `RawWriter::append`（同じ測定 dir に trial を足す。`run.json` が有れば拒否）。
- `tests/dagbench_harness.rs`: `Params` に `backend`（既定 ext-sim）・`recovery`・`pbs`（`DAGBENCH_PBS_{SPEC,STUDY,TRANSPORT,STATE}`）、
  `run_from_env` が `DAGBENCH_BACKEND=pbs` で `run_pbs`（1 起動 = 1 trial）へ、子の入口 `pbs_control_child`。
  ext-sim の `run` は recovery/pbs 欄を拒否するだけで挙動不変。
- `tests/common/dagbench_pbs_controller.rs`（新規）: 子の `CelerisChildController` と親の読み直し
  （baseline `state.json` の再 open、Celeris `SQLITE_OPEN_READ_ONLY` で `cluster_job_waits`）。
- 試験 `harness_pbs_` 6 本: 両 arm × normal/A2 の 4 本、4 trial を同じ測定 dir に足して `raw::validate`（real_jobs 4）と
  trial 再実行の拒否、入口の組合せ拒否。各 trial で物理 qsub 1（`calls.jsonl`）、消費 1、消費 job_ref = 保存済み Waiting の job、
  A2 は停止中の照会が runner の pid・再開後の照会が incarnation 2 の pid、停止中原票が同じ `Job Id` で `F`。

## 証拠（提出時の HEAD で実行）

| 検査 | 結果 |
|---|---|
| `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_` | exit 0、6 passed |
| `cargo test -p celeris-campaign --test dagbench_harness --test dagbench_pbs_e2e` | exit 0、harness 19 passed / 3 ignored、e2e 5 passed |
| `bash scripts/dev/test-affected.sh` | exit 0、2691 passed / 7 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## 未解決事項（後続 unit へ）

- I14 (2) の「e2e の in-memory `Cluster` を `FilePbs` に置き換えて消す」と (3) の「in-process A2（`entry::run_trial` の `Gate`
  `on_stop`）を消す」は未実施。本 unit の受け入れ条件 1（`dagbench_pbs_e2e` 不変で通る）を優先した。harness-close で
  `dagbench_pbs_e2e` を `run_pbs` 経由へ移してから旧経路を消す。`CelerisController`（e2e）も同時に common へ寄せる。
- `run.json` を作成時 1 回にして `real_jobs` を再集計へ移す変更（I14 (1)）は harness-raw。現状は全 trial の後に
  `RawWriter::append(..).write_run(..)` で閉じる（試験 `harness_pbs_both_arms_and_conditions_share_one_measurement`）。
- `recovery-trials.jsonl`・壁時計/論理時計の欄（(4)）、`RejectReason` 型と validator の再判定・`DAGBENCH_PBS_CHILD_FAULT`（(5)）、
  `regenerate`（(6)）は harness-raw / harness-reject。現状の拒否は `wait_not_saved:` 等の理由名で始まる Err 文字列。
- `FilePbsFaults` は全欄を実装済みだが、故障の試験は harness-reject で足す。

## 提案

- なし。
