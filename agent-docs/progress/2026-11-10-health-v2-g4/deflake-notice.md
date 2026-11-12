---
title: 健全性v2 G4 deflake-notice — dagbench_process recovery_notice_continuation の hang の調査（plan_issue）
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: abandoned
updated: 2026-11-11
---
# 健全性v2 G4 deflake-notice — dagbench_process recovery_notice_continuation の hang の調査（plan_issue）

完了日: なし（2026-11-11 の 2 回の run はどちらも plan_issue で終え、計画 v6 で G4 から外した）

## 結論
- 計画 v6 で G4 から外した（unit の状態は superseded）。G4 の branch は `crates/campaign` を変えていない。原因の調査は G4 の外の別 task にする（下の「提案」の 2.、G4 の進捗の「提案」）。以下は 2 回の run の調査の記録で、写しの元は task の成果物 `deflake-notice-record.md`。
- 2 回の run で、hang に至る順序を特定できなかった。再現試験 `notice_stall_*` と修正は入れていない。repo の変更はこの記録だけ。
- 前回の run が疑った順序（c1 が先に待機した job の wait を保存する前に止まり、c2 が 2 node を再実行する）は、この run の 1 回で accept・wait の両方に現れ、どちらも合格した（下の「再現の試み」）。この順序は hang の原因ではない。
- 失敗した log からは「他の試験が終わった後も単独で長く走った」ことを確かめられない（下の「根拠にした失敗」）。負荷による遅さ（同じ log の celeris-campaign の試験 20 件が SLOW）の見込みも残る（未確認）。
- 待ちに上限を足して落とすだけ・retry・ignore・試験の分割による回避はしていない。

## 根拠にした失敗
- `<workspace>/integration-checks/integrate-hardening/1794367017446-7.log`。`recovery_notice_continuation` は 3737/3737 番目に 600.018 秒で TIMEOUT。Summary は 743.547 秒なので、この試験は nextest の開始から 143 秒以降に始まった。
- 同じ log の SLOW は 28 件で、うち 20 件が celeris-campaign（dagbench_harness・dagbench_overhead・dagbench_pbs_backend が 61〜206 秒、k4_cells_validity が 335 秒）。直前に終わった 3 件は project_controller_soak_72h（各 100〜109 秒）。nextest の出力には試験ごとの開始・終了の時刻が無いので、この試験が単独で走った時間は分からない（未確認）。
- 試験の stdout は `has been running for over 60 seconds` だけで、6 つの sub-run のどこで止まったかは log から分からない。raw は tempdir で、残っていなかった。

## 調べた待ち（`crates/campaign/tests/dagbench_process.rs` と `crates/campaign/src/dagbench/`）

| 待ち | 場所 | 上限 |
|---|---|---|
| `wait_ready`（External の ready 行） | 試験 | 行ごとに 60 秒 |
| `wait_line(&c1, "CONTROL_ACKED")` | 試験 `notice_run`・`no_notice_probe` | 行ごとに `INSURANCE` = 600 秒（nextest の打ち切りと同じ） |
| `wait_line(&c2, "CONTROL_RESUMED")` | 試験 | 600 秒。c2 は起動直後に出す |
| `c2.wait_exit(INSURANCE)` | 試験 `notice_run` | 600 秒 |
| `c2.wait_exit(PROBE_BOUND)` → kill | 試験 `no_notice_probe` | 60 秒 + `kill_wait` 20 秒 |
| IPC の 1 要求 | `ipc.rs` `IpcExt::call` | `(retries+1) × io_timeout` = 240 秒。server は接続ごとの thread |
| server の `Receive` | `ipc.rs` | `wait_ms`（50 ms）。保留の通知があるときは 0 |
| 保留（`hold`）した通知 | `ipc.rs` `NoticeChannel` | ack せず手元に持ち、次の `receive` で必ず渡し直す（取りこぼさない） |
| celeris 制御 `CelerisSide::drive` の `None if waiting => {}` | `celeris_side.rs` | 壁時計の `cap_wall`（600 秒）だけ。`waiting` の wait の通知が適用されないとここを回る |
| tokio runtime の drop（c2 の成功時） | 試験 `control_role` | `CelerisSide::drop` が論理時計の sleeper を起こしてから runtime が落ちるので、`spawn_blocking` の sleeper は残らない |
| baseline 制御 `Baseline::run` | `baseline.rs` | 上限なし。ただし制御は 1 thread、ext-sim の job 表は `BTreeMap` で、重複通知が付く job も毎回同じ。順序は決定的で、この run でも合格 |

通知が適用されないまま ack される経路（`pump_notices` の `processed`・`logical_consumed`・`Duplicate`・`UnknownJob`）も読んだ。この run の raw では、これらに当たったのは重複 copy（copy 1）だけだった。

## 再現の試み（負荷は掛けていない）
- `cargo test -p celeris-campaign --test dagbench_process --no-run`（exit 0）の後、`DAGBENCH_PROCESS_RAW_DIR=<artifacts>/raw1 <binary> --exact recovery_notice_continuation --nocapture` を 1 回実行: exit 0、1 passed、28.14 秒（load average 約 24〜31）。
- celeris の accept と wait の両方で、c1 は先に待機した exp-posix の wait を保存しないまま kill され、c2 は exp-uring・exp-posix の 2 node を再実行した（ext ledger の `submit_deduped` が 2 件）。どちらの通知も wait ができるまで `notice_held` で保留され、wait ができた後に `finished` で適用された。重複 copy は `duplicate` で抑止された。
- 前回の run の合格例では、c1 が exp-posix の wait を保存し、c2 は exp-uring だけを再実行した。これで c1 の止まり方の 2 通り（wait の保存の有無）はどちらも合格を確かめた。
- このため、SIGSTOP/SIGCONT や遅延フックで固定すべき「hang に至る順序」の候補が無く、`notice_stall_` の試験は作っていない（直す前に落ちる試験を書けない）。

## 証拠コマンドと結果（提出時の HEAD で実行）
- `git diff --name-only "$CELERIS_WU_BASE"`: この記録の 1 file だけ（crates の変更なし）。
- `sh scripts/dev/progress-index.sh --check`: 結果は報告本文に書く。
- test-affected・clippy・fmt: crates を変えていないので実行していない（記録だけの変更）。

## 未解決事項
- hang に至る順序が未特定。受け入れ条件 0〜2（原因の記録・`notice_stall_` の再現試験・dagbench_process の全合格の確認）は満たしていない。
- 失敗が hang か負荷による遅さかも未確定。

## 提案
1. G4 の branch は `crates/campaign` を変えていないので、段 hardening の統合 check を同じ HEAD で再実行する（deflake-notice を G4 の計画から外す）。
2. 原因を突き止める仕事は G4 の外の別 task にする。最初に、TIMEOUT のときにどの sub-run のどの待ちで止まったかが分かるようにする（試験の各段 — sub-run の開始・`CONTROL_ACKED`・observer の完了・c2 の起動と終了 — を stderr に時刻付きで出す。統合の check で `DAGBENCH_PROCESS_RAW_DIR` を作業場所の下に向け、raw を残す）。これは診断で、直しではない。
3. 試験の `INSURANCE` と celeris 制御の `cap_wall` が nextest の打ち切りと同じ 600 秒なので、試験自身の失敗文が出る前に TIMEOUT になる。原因が分かった後に、決定的な再現試験と合わせて見直す。
