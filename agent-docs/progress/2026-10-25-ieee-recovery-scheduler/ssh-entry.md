---
title: IEEE recovery scheduler — ssh 経路の承認 gate・SshSite・close_from_env（ssh-entry）
tasks: [01M5TCGEVJF1QW03CDWPJRRKZ5]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# ssh 経路の承認 gate・SshSite・close_from_env（ssh-entry）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。ADR 付記 I15 (1)〜(3) を実装した。
D1–D5・I1–I14 の挙動（file transport）は変えていない。実 ssh・network・qsub・LLM は使っていない。

## やったこと

- 新規 `crates/campaign/src/dagbench/pbs/approval.rs`（I15 (1)）
  - `load`: 承認記録 `celeris.recovery-smoke-approval/1`（`deny_unknown_fields`）を読み、絶対 path・`source` 非空・
    cluster/queue/account/select/place/walltime_secs/measurement_id の spec との完全一致・`1 ≤ max_jobs ≤ spec.limits.max_jobs`・
    `valid_until` が `Z` の RFC 3339 で起動時の UTC より後、を確かめる。
  - `pin` / `check_pinned`: 測定 dir の `recovery-approval.json` に hard link で 1 回だけ作り、以後は sha256 一致を求める。
  - `ApprovalGate<T>`: qsub ごとに「UTC < valid_until」と「intent 台帳で `sent` 以降に進んだ intent（自分を除く）の数 <
    min(spec, 承認) の max_jobs」を確かめ、外れれば qsub を起こさず `CallOutcome::Unsupported` の原票を返す。
    qsub は台帳 lock の中で呼ばれるので、件数は lock を取らずに台帳 file を読む。qstat・marker 検索は素通し。
- `crates/campaign/src/dagbench/pbs/proc.rs`: `ChildConfig.state` を `PbsAccess`（`File { state }` / `Ssh { approval, … }`）に
  置き換えた。transport・時計・Site・論理欄の時刻を `PbsAccess` から作り、親と子がそれぞれ承認を読み直して pin と照合する
  （子は親の検査結果を信じない）。ssh では子の故障 env（`DAGBENCH_PBS_CHILD_FAULT`）を拒否する。
- `crates/campaign/src/dagbench/pbs/transport.rs`（I15 (2)）: `SshSite`（`impl entry::Site`）。cluster dir は
  `cluster_measurement_dir`（絶対・`..`/`.`/空要素なし・`[A-Za-z0-9._/-]`・basename = measurement_id）で検査し、job ID は
  `is_scheduler_job_id` を通るものだけ。`release` = `touch -- <dir>/release/<id>`、`fetch_outputs` = `cat -- <dir>/out/<id>/out.bin`
  （1 MiB 上限・空/非 0/timeout は Err・`dest/out.bin` は新規作成）。argv は `SshPbs::ssh_argv` と同じで全体 timeout を持つ。
  呼び出しごとに trial dir の `site-calls.jsonl`（`celeris.recovery-site-call/1`）へ 1 行。byte のまま読むため `run_bounded` を
  `run_bounded_bytes` に分けた（`PbsReply` の作り方は不変）。共有のため `impl PbsTransport for Arc<T>` を足した。
- `crates/campaign/src/dagbench/pbs/raw.rs`: `validate` が `site-calls.jsonl` のある trial で `attempt-1/out.bin` の sha256 と
  最後の成功した `fetch_outputs` 行の `stdout_sha256` の一致を確かめる（file transport の trial は対象外）。
- `crates/campaign/tests/dagbench_harness.rs`
  - `run_pbs`: `file` は state 必須・承認を拒否、`ssh` は承認必須・state を拒否。ssh は測定 dir を作る前に承認を検査し、
    外れれば何も作らず ssh を起こさない（既存試験が見る `file only` の文言は承認なしの ssh の Err に残した）。
  - `PbsParams.state` を `Option`、`approval` を追加（`DAGBENCH_PBS_APPROVAL`）。
  - `#[ignore]` の `close_from_env`（`DAGBENCH_CLOSE_RAW`・`DAGBENCH_CLOSE_PINS`。`write_run` を 1 回、`validate`、`RunMeta` を
    stdout に。2 回目は `run.json` が既にあるので Err で何も書かない。transport を開かない）。
  - 試験 5 本（`harness_pbs_ssh_*`）。stub の ssh（qsub は job を作るだけ、qstat は解放 marker の有無で R/F、`touch`/`cat` は
    手元の cluster dir でその場実行）を使う。
- `docs/architecture-map.md` の campaign::dagbench::pbs 行に approval・`PbsAccess`・`SshSite`・`close_from_env`・
  `harness_pbs_ssh_*`・I15 を足した（6 列のまま）。

## 検査（提出時の HEAD で実行）

- `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_` → exit 0、20 passed（うち `harness_pbs_ssh_` 5 本:
  approved_normal_and_a2_pass_for_both_arms / refuses_missing_mismatched_expired_or_oversized_approval /
  gate_refuses_qsub_beyond_max_jobs_and_swapped_approval / site_argv_uses_only_checked_paths /
  site_timeout_is_classified_as_timeout。既存 15 本は無変更で通過）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch ほか、
  nextest 2705 tests run: 2705 passed, 9 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- commit: 「ssh-entry: approval gate, SshSite and close_from_env for the ssh PBS path (ADR I15 (1)-(3))」

## 未解決事項

- 試験専用の差し替え: rcv-1 の DAG は 180 論理秒あり、実時計では stub の試験が実時間を数分待つ。そこで試験だけ
  `PbsParams.ssh_test`（`SshTest { program, clock_state }` → `PbsAccess::Ssh { program, test_clock }`）で stub の ssh の path と
  偽の論理時計（`FileClock`）を渡す。env 入口（`run_from_env`）は常に None で、本番の ssh は PATH の `ssh` と実時計
  （I15 (1)「時計は実時計で、FileClock は使わない」は本番の経路では守られる）。試験時の原票の時計種別は `file_fake` になる。
  objective の「PATH 先頭に置いた stub」は、親の process の PATH を試験中に書き換えない（他の試験と競合する）ため、
  stub の path を明示で渡す形にした。
- I15 (4) の `QsubRequest::argv` に `RECOVERY_SMOKE_DIR=dirname(script)` を足す点は本 unit の範囲（(1)〜(3)）外で未実装。
  実機 smoke（live-smoke）の前に、payload の統合（integrate-impl）か runbook で入れる必要がある。
- gate の拒否は ADR どおり `Unsupported`（照合不能として止まる）。送信前に確実に拒否したので `SpawnFailed`
  （RejectedBeforeSend）の方が意味は正確。

## 提案

- I15 (4) の argv 追加を integrate-impl の修正葉にする（file/ssh 両方の `Variable_List` 期待値と `FilePbs` の原票も合わせる）。
