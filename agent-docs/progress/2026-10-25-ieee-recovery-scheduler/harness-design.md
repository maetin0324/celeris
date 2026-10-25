---
title: IEEE recovery scheduler — 正式入口接続・別 process 停止・raw 指標・拒否条件の設計固定（harness-design）
tasks: [01M5T2P8MAW1564BFKSXXV33YG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 正式入口接続・別 process 停止・raw 指標・拒否条件の設計固定（harness-design）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。コード（`crates/`）は変えていない。

## やったこと

- [ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) に付記 I14 を追記した（D1–D5・I1–I13 の本文は不変）。
  読んだもの: `crates/campaign/tests/dagbench_harness.rs`（`run_from_env`・`child_control`・`run_faulted`）、
  `dagbench/fault.rs`（`FaultRunner` の SIGKILL）、`dagbench/process.rs`（`Supervisor`・`OwnedProc`・`EventLog`）、
  `tests/common/dagbench_durable.rs`（ext-sim の独立 process A2）、`pbs/entry.rs`（in-process A2）、`pbs/raw.rs`、
  `pbs/transport.rs`（`FakePbs`）、`tests/dagbench_pbs_e2e.rs`（in-memory `Cluster`・`CelerisController`）。
- I14 で決めた形:
  1. 入口 env `DAGBENCH_BACKEND=ext-sim|pbs`（既定 ext-sim）・`DAGBENCH_RECOVERY=normal|a2`（pbs のみ）・
     `DAGBENCH_PBS_{SPEC,STUDY,TRANSPORT,STATE}`。1 起動 = 1 trial、測定 dir は共有、trial dir は新規のみ。ext-sim と既存 fault id は不変。
  2. `pbs/file_pbs.rs` の `FilePbs`（`PbsTransport` + `Site`、`jobs.jsonl`・`calls.jsonl`・`outputs/`・`faults.json`）と `FileClock`。
     e2e の in-memory `Cluster` は置き換えて消す。
  3. `pbs/proc.rs`（親 `ProcTrial`・子 `child_trial`、子の入口 `pbs_control_child`、`PBS_WAIT_SAVED` 行と stdin `CONTINUE`）。
     親は state.json の再 open / `SQLITE_OPEN_READ_ONLY` で Waiting を確かめ、starttime 照合後に `OwnedProc::kill`。
     停止中は親の observer が qstat 原票を raw に記録、incarnation 2 で同じ state から回収。重複投入・消費の検出規則。
  4. `recovery-trials.jsonl`（`celeris.recovery-trial/1`、`RecoveryRecord`）: 復帰時間（`wall_*`）・論理時計（`logical_*`、`clock_kind`）・
     `rework`・`human_intervention`（op 記録なしは null）・`scheduler_class`（unknown/fetch_failed は成功にしない）・`recovery_status`。
  5. `RejectReason`: `wait_not_saved`・`saved_evidence_missing`・`job_mismatch`・`duplicate_submit`/`duplicate_consume`・`stop_boundary_invalid`。
  6. `pbs/metrics.rs` の `regenerate(raw, out)` と ignored 入口 `regenerate_from_env`。新規 out のみ・旧 raw/main-3/K4 は版で拒否・
     raw の sha256 一覧の不変と 3 回の出力の byte 一致を試験で assert。

## 証拠

commit「docs: add ADR addendum I14 for formal entry, process stop, raw metrics and rejections」の HEAD で実行（提出時の HEAD）。

- `grep -c '^### I14' agent-docs/adr/2026-10-25-recovery-scheduler-backend.md` → 1。
- 範囲 check `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR と本 file の 2 path のみ（`crates/` なし）。
- 下の「検査」節を参照。

## 未解決事項

- 実 `SshPbs` での入口（`DAGBENCH_PBS_TRANSPORT=ssh`）は人の承認・認証・残枠確認まで拒否のまま。
- `human_resolution` 行（I13）の形式は未決のまま。

## 提案

なし。
