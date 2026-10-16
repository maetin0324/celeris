---
title: 監査修正4 watch-driver（継続監視/再開 driver と job 終了確認）
updated: 2026-10-16
status: done
tasks: [01M530Y2NMKB1PAAXYH1W77893]
---

# 監査修正4 watch-driver

設計は [ADR](../../adr/2026-10-16-campaign-watch-driver.md)。完了日: 2026-10-16。

## 着手時の確認

err-taxonomy / budget-reserve は main に入っている（base cbf07304）。残っていた項目: driver が無い、`CelerisExecutor::stopped` が既定 true
（cancelled task + live job で予約を解除し得る）、cancel の所有判定が無い、cluster job の walltime が task に伝わらない。これらを修正した。

## 変更

- `watch.rs`（新規）・`main.rs`: `celeris-campaign watch`。`Clock` 注入。
- `celeris_exec.rs`: `stopped`（task 木 + job 終端）、poll の timeout は停止確認まで Wait、cancel の所有判定（label `campaign-<hex>`）、
  objective と cancel 台帳に walltime。
- 試験: `tests/watch_driver.rs`（5 件: 再走査・再起動再開で再 submit なし・通信断・停止未確認・不正引数）、
  `tests/celeris_exec.rs`（job live は完了扱いにならない、無関係 task/job を cancel しない、walltime）、`tests/cli_executor.rs`（watch CLI）。

## 証拠

- `cargo test -p celeris-campaign --no-fail-fast`: 全 pass。
- 全体 gate は報告本文に記載。

## 未解決事項・提案

- cluster job の cancel は従来どおり人手手順の記録（自動 qdel なし）。本測定で job を使う場合は runbook（closeout）に手順を書く。
- 実 token 上限の task への受け渡しは未実装（budget-reserve の提案を継続）。
