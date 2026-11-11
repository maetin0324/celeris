---
title: クラスタ job の失敗・長い Q/H による早期再開
tasks: [01M752V5864CMYZZ2QHYAJ4X0E]
status: done
updated: 2026-11-11
---
# クラスタ job の失敗・長い Q/H による早期再開

## 設計 brief

1. 根本原因: wait の再開判定が全終了だけを条件とし、部分失敗と長い Q/H を worker が調べる機会が無かった。
2. 責務 owner: 地図の durable wait 行にある task-core::cluster_job（状態解析・起床条件）、task-dispatch::dispatcher::cluster_job_wait（poll と再開）、task-worker（前置き）。
3. 既存の再利用先: ClusterJobWaitFinished event、ClusterJobResume、needs_continuation と既存の偽 poller・注入時計。
4. 選択肢: worker を動かしたまま監視する案は採らず、既存 durable wait の再開条件を拡張する。監視で LLM と run の枠を消費しないため。
5. 新しい状態・分岐・fallback・port: status JSON に Q/H の初回観測時刻と通知済み情報を追加。新しい wait 状態・DB 列・fallback・port は無し。
6. 撤去対象: 全終了だけに限定する再開判定と説明を置き換える。前置き組立ては worker の共通経路へ移し、dispatcher の旧本体を削除。
7. 不変条件: events 追記専用、task の正本は SQLite、自動 qdel/scancel 禁止、LLM を dispatcher に置かない。全成功・期限超過・計数除外の既存試験と、下記の追加試験で確認する。

完了日: 2026-11-11

## 変更

- commit 件名: `phase 1: wake cluster job waits on failure and queue stalls`

- 部分失敗と Q/H の観測時間を再開条件に追加。既存の wait 終了 event、task/WU continuation を使う。
- `queued_wake_secs`（既定 3600 秒）と検証を追加。通知済み情報を status JSON に保存し、Q/H の起床は同じ job の待ち直しでも繰り返さない。失敗の起床は抑制しない。
- PBS の資源指定・comment・estimated.start_time を折返しも含めて引き継ぐ。
- continuation の組立てを task-worker に移動。worker の指示と設定例を更新。
- ADR: `2026-11-11-cluster-job-early-wakeup.md`。

## 試験の不変条件と追加理由

全終了の既存試験では、失敗の後も他 job を待ってしまう問題、Q/H の長期化と待ち直し時の通知重複を検出できなかった。
偽 poller と注入時計で、atomic・v2/v3 WU の早期起床、全 job と理由の前置き、
dispatcher 再作成、Q→H、再待機、continuation 回数不変を確かめる。
Slurm は状態が FAILED 等で exit 0 の場合と、exit 0 だが signal 非ゼロの場合も失敗とする。

## 検査環境と補修

- sandbox では共有 runner cache が書込不可のため `CELERIS_SCRIPT_RUNNER_CACHE=$TMPDIR/script-runner-cache` を使う。
- pnpm 12 の offline store は `PNPM_CONFIG_STORE_DIR=$PWD/.pnpm-store` で検証用の写しにも引き継ぐ。
  offline install の不足を復元後、小さな別ディレクトリで offline install が通ることを確認した。
- 既存の起動通知試験は Codex stub の設定に一時ホームを渡すよう修正。呼出元のアカウントの書込権限への依存を除き、通知の順序・回数・失敗時の assertion は維持した。
- EventRow・API・worker protocol の schema と web の生成型を更新。

## 証拠

提出時の HEAD で実行。基準は `git merge-base HEAD main`。最後の同期後にソースを変更していない。

- 条件 0・2: `bash scripts/dev/test-affected.sh` → exit 0。偽 poller の `partial_failure_resumes_atomic_with_every_job_and_can_wait_again` と `partial_failure_resumes_v2_and_v3_work_units` が合格。3 job の部分失敗・Q/R・理由の前置き、再待機、計数除外を確認。
- 条件 1: 同コマンド → exit 0。`queued_job_wakes_once_across_rewait_with_scheduling_evidence` と従来の成功・上限試験が合格。資源指定・comment・開始見込み、Q/H の通知一度、dispatcher 再作成後の継承を確認。
- 条件 3・4: worker の共通指示、継続の前置き、ADR と設定例を確認。文書検査 4 本は exit 0。
- 条件 5: `bash scripts/dev/test-affected.sh` → exit 0。Rust 8224 passed / 0 failed / 23 skipped（907.019 秒）、対象 doc test 合格、web の型検査・unit 712 件合格、test-ts-verify 20 passed / 0 failed、script bundle SKIP 0。
  sandbox の検証では `NEXTEST_TEST_THREADS=8`、`PNPM_CONFIG_STORE_DIR=$PWD/.pnpm-store`、`CELERIS_SCRIPT_RUNNER_CACHE=$TMPDIR/script-runner-cache` を指定。ログは run artifacts の `acceptance-submit.log`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`、`python3 scripts/dev/check-architecture-map.py` → exit 0。
- `git diff --check` → exit 0。

## 本番への移行

`queued_wake_secs` は、そのキーを知る release の昇格後に人が本番設定へ追加する。
省略時は 3600 秒。BenchFS の task、本番 job、本番設定は操作しない。

## 未解決

無し。実クラスタの job 操作は実行していない。

## 提案

変更範囲外の `celeris-credentiald::injection_ipc::concurrent_first_request_streams_over_the_limit_are_denied_unread`
が全体実行で一度 `call: Io` になった。コードを変えない単独再実行は 1 件合格（0.03 秒）。
待機枠を解放した後の接続の拒否と client の読み方は別 task で調べる候補とする。原因は未確定。
credentiald のコード・試験は変更せず、最終の影響範囲試験は `NEXTEST_TEST_THREADS=8` で再実行し、8224 件すべて合格した。

