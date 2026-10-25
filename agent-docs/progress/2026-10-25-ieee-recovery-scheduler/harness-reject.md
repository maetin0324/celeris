---
title: IEEE recovery scheduler — 正式入口の拒否条件を独立 process の決定的試験で固定（harness-reject）
tasks: [01M5T2P8MAW1564BFKSXXV33YG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 正式入口の拒否条件を独立 process の決定的試験で固定（harness-reject）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。ADR 付記 I14 (5) を実装した。
commit: 「harness-process: close the child stdin only after SIGKILL so an immediate kill is never pre-empted by an EOF exit」、「harness-reject: refuse stop-before-save, missing saved evidence, job mismatch, duplicate submit/consume and unknown scheduler on the formal pbs entry」。

## やったこと

- `pbs/proc.rs`: 子の試験専用故障 `ChildFault`（env `DAGBENCH_PBS_CHILD_FAULT` = `announce-before-save` / `duplicate-submit` /
  `duplicate-consume`）。`duplicate-submit` は `CONTINUE` の後に別 intent（`workload_attempt`+1）で同じ request を投入し直す、
  `duplicate-consume` は完走後に消費行を台帳の重複防止を迂回して 2 行目として足す。
  `ProcTrial::run` は Err（拒否）で trial dir に `reject.json`（`celeris.recovery-reject/1`、`reject_reason` は Err の理由名を
  `RejectReason` に読んだもの、`detail`）を新規作成する。停止中の原票は `IntentSpec::check_record` で照合し、Variable_List 等の
  食い違いは `job_mismatch`（再起動しない）。
- `dagbench/process.rs`: `OwnedProc::kill`（即時 kill）は SIGKILL の後に stdin を閉じる。従来は先に閉じたため、`CONTINUE` を
  待つ子が EOF で自分から exit 101 し、負荷下で `exit_signal` 9 にならず A2 が `stop_boundary_invalid` で落ちた
  （test-affected で `harness_pbs_both_arms_and_conditions_share_one_measurement` と本 unit の試験が再現、単独実行では通る）。
  graceful 停止（`stop`）は従来どおり stdin を閉じて待つ。
- `pbs/entry.rs`: `child_gated` に `announce_early`。`GatedExt::submit` が job ID を受けた直後（保存前）に合図する（故障時のみ）。
- `tests/dagbench_harness.rs`: `PbsParams.child_fault`（入口で `ChildFault::parse`、子の env にだけ渡す）と試験 `harness_pbs_reject_` 5 本
  （全て `run_pbs` = 正式入口、偽 PBS は `FilePbs`、controller は別 process）:
  1. `stop_before_save_is_refused_without_kill`: 両 arm × A2、保存前の合図 → `wait_not_saved`。barrier なし・controller の
     `process_started` 1 回（再起動なし）・`durable-wait.json` なし・job 未解放・消費 0・qsub 1。
  2. `missing_saved_evidence_is_not_success`: 両 arm × normal の保存前の合図 → `wait_not_saved`（`CONTINUE`・解放なし）。閉じた raw の写しで
     `durable-wait.json` 削除（missing）・書換え（altered）・recovery 行の `saved_wait` null を validator が拒否、`regenerate` も拒否。
  3. `job_mismatch_after_restart`: `FilePbsFaults.variable_list_trial_key` で両 arm × A2 → `job_mismatch`（`controller_resumed` なし・消費 0）、
     normal は `Recovered` にならない。
  4. `duplicate_submit_and_consume`: 両 arm、`duplicate_submit`（物理 qsub 2 を検出）・`duplicate_consume`（2 行）・未知の故障名は入口で拒否（qsub 0）。
  5. `scheduler_unknown_is_not_success`: `forget_after_release`（Unknown Job Id）と `qstat_outcome=Timeout`（collector 障害）× 両 arm × normal/A2 で、
     Ok なら `recovery_status != Recovered`、Err なら `reject.json` があり `a2.json` が無い。
- 待ちは子の合図・exit の出来事待ちだけ（sleep・CPU 負荷なし）。これらは無 LLM・無実 job の偽 transport での決定的試験で、
  LLM の未知修復能力・HPC 実測とは称さない。

## 証拠（提出時の HEAD で実行）

| 検査 | 結果 |
|---|---|
| `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_reject_` | exit 0、5 passed |
| `cargo test -p celeris-campaign --test dagbench_harness --test dagbench_pbs_e2e` | exit 0、harness 28 passed / 4 ignored、e2e 5 passed |
| `bash scripts/dev/test-affected.sh` | TEST_AFFECTED |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## 未解決事項（harness-close へ）

- `reject.json` は trial dir の記録で、`recovery-trials.jsonl` に `recovery_status=rejected` の行は足していない（拒否は collect の前で
  `TrialBinding` が無いため）。validator は `reject.json` を読まない。ADR 付記 I14 (5) の「raw に rejected と理由を残す」との差を突き合わせる。
- `wait_not_saved` の子の停止は `OwnedProc::kill`（合図の stdin 待ちで止まっている測定専用 process を止める）。A2 の
  `controller_killed` barrier は残さない。
- `collector_failed`・`unobserved` は `RejectReason` に無く、Err の理由名のまま（`reject_reason` は null、`detail` に残る）。

## 提案

- なし。
