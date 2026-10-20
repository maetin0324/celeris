---
title: continuation 上限を進捗に合わせる（ADR-0072 D18 の改定）
updated: 2026-10-20
tasks: [01M5C6M6JCKFNDGHP3ZXBVPMR2]
status: running
---
# continuation 上限を進捗に合わせる

ADR: `agent-docs/adr/2026-10-20-continuation-limit-by-progress.md`

## 工程
- [x] fix-docref（統合済み）
- [x] adr-b: ADR を書く（2026-10-20。コードは触らない）
- [ ] impl-b: 判定・config・試験の実装（ADR の D1〜D5 と「試験」節）
- [ ] 統合

## adr-b の結果（2026-10-20）
- 現状を base 669c00ab のコードで確認して ADR の「現状」節に書いた。要点:
  - task 経路 `worker_finish.rs:1094` と WU 経路 `execution_scheduler.rs:195` の両方で、回数の上限（既定 3）が進捗を見ずに
    人に聞く。文言はどちらも「進みません」。
  - WU の `wu.continuations` は人の回答で 0 に戻らない（D18「回答の時点から数え直す」と食い違い）。「続ける」と答えても次の
    continuable な終わりで再び止まる。
  - WU の `blocked(limit)` は replan に回る（「made no progress」）。回数で止まっただけの WU も replan される。
- 決定: 回数だけの上限を消し、進捗なし（`no_progress_limit`、既定 2）と固い上限（新欄 `continuation_hard_limit`、既定 12）
  だけで人に聞く。固い上限の文言は回数・進捗あり回数・commit 数・変更 file 数を示す。WU は新しい
  `WorkUnitBlockedReason::ContinuationCap`（replan しない、回答で `needs_continuation` から再開）。旧欄
  `max_continuations_per_work_unit` は読み込みエラー。

## 証拠
- `git diff --name-only $CELERIS_WU_BASE` → ADR と本ファイルの 2 件だけ（コードの変更なし）。
- 本番 config の確認（読み取りのみ）: `grep -n "^\[execution\]" -A12 ~/.config/celeris/config.toml` → `[execution]` は
  `parallel` と `gate` だけ。`max_continuations_per_work_unit` も `no_progress_limit` も書いていない。

## 本番への移行
- impl-b を含む release を昇格する前に、運用者が `grep -n max_continuations_per_work_unit ~/.config/celeris/config.toml` を
  実行し、何も出ないことを確かめる（2026-10-20 時点では出ない）。出たらその行を消す（残すと読み込みエラーで daemon が起動しない）。
  旧い値を固い上限として残したいときだけ `[execution] continuation_hard_limit = <n>`（1 以上）を書く。
- DB の migration は無い（`work_units.blocked_reason` は TEXT 列。新しい値 `continuation_cap` が入るだけ）。

## 未解決事項
- 固い上限の既定 12 は運用の実績から決めた値ではない（1 run の予算 × 13 run ≈ 9 時間弱で暴走を止める見積もり）。運用で
  上限到達の質問が多ければ見直す。
- tokens の費用上限は足していない（ADR「採らなかった案」）。

## 提案
- ADR-0072 D18 の表と ADR-0090 の `max_continuations_per_work_unit` への言及に、本 ADR で置き換えた旨の付記を impl-b で足す。
