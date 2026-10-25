---
title: IEEE recovery scheduler — PBS transport（pbs-transport）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# PBS transport（pbs-transport）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) 実装付記 I1・I2 に従った。実 scheduler・ssh・qsub・外部 network には通信していない。

## やったこと

- `crates/campaign/src/dagbench/pbs/{mod,transport}.rs` を追加（`dagbench/mod.rs` に `pub mod pbs;`）。
  - `PbsTransport`（`origin` / `qsub` / `qstat_full` / `find_by_marker`）と原票型 `PbsReply`（call・argv・開始/終了 UTC・exit・stdout・stderr・outcome = Ok|Timeout|SpawnFailed|NonZero|Unsupported）。
  - `QsubRequest::argv`: `-q -A -l select/place/walltime -r n -N -v CELERIS_TRIAL_KEY=..,CELERIS_SUBMIT_INTENT=.. <script>` を spec の欄だけから組み、文字種を検査（外れれば起動せず SpawnFailed）。
  - `SshPbs`: `ssh -o BatchMode=yes -o ConnectTimeout=<n> -o ControlMaster=no -- <cluster> ...`（既存 master は使い、起動・停止しない）。1 回の呼出し全体の timeout で SIGKILL・回収し、途中出力を残す。待ちは `CallTimer`（既定は実時計、試験で注入）。`find_by_marker` は D4.4 未確認のため起動せず `Unsupported`。
  - 分類: `classify_qsub` → Accepted（完全な PBS ID 1 行のみ）/ RejectedBeforeSend（起動していない）/ Unknown{Timeout・Disconnected・NonZero・InvalidId・Unsupported}。`classify_qstat` → Record / UnknownJob / Failed。`classify_marker` → Candidates（owner と両 marker の完全一致）/ Failed。
  - `QstatView`: qstat_full だけを `observe::SchedulerTransport` に写す view（qsub を呼べない）。原票を溜める。
  - `FakePbs`: call 種別ごとの台本、物理投入数などの `FakeCounts`、合成時計。
- 共用: `observe::parse_qstat` を `pub(crate)` に上げ、`Variable_List` の値取り出しを `observe::job_variable` に切り出して `observe_job` と共用。cluster の安全文字検査を `ssh_qstat::is_safe_cluster` に切り出し `SshQstat` と `SshPbs` で共用。
- `tests/runner_checker.rs` の src 走査を `dagbench/pbs/` まで広げた（未知のサブディレクトリは引き続き失敗）。
- 試験 `crates/campaign/tests/dagbench_pbs_transport.rs`（8 件）: qsub 要求の `-r n`・両 marker・不正引数、qsub 分類（timeout・切断・非 0・PID 形・空・複数行 = unknown、起動失敗のみ送信前拒否）、qstat 分類、marker 完全一致（他人・別 trial・別 intent・前方一致を除外）、QstatView、stub ssh の argv、全体 timeout（stub が途中出力後に fifo で知らせた出来事で注入時計を発火。実時間は待たない）。

## 証拠

提出時の HEAD で実行（結果は本 WU の報告に記す）。

- `cargo test -p celeris-campaign --test dagbench_pbs_transport` → 8 passed。
- `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`。

## 未解決事項

- marker 検索の実 command は接続後の read-only 確認まで未定（D4.4）。`SshPbs::find_by_marker` は Unsupported。
- `pbs/mod.rs` は並行の submit-intent WU も作る可能性があり、統合時に `pub mod` 行の併合が要る。
- qsub の非 0 終了（exit≠255）も unknown 扱い（PBS の拒否と届かなかったことを区別しない）。送信前拒否は起動していない場合だけ。

## 提案

なし。
