---
title: コメントで中断した run の続きも同じ claude session を resume する（ADR-0140 付記 comment-resume）
tasks: [01M42CKKJ76Q1T3F72QK11SGH7]
status: done
updated: 2026-10-04
---

# PROGRESS — コメント中断の続きの session resume

正本: [ADR-0140](../adr/0140-claude-session-resume.md) の「付記（2026-10-04、comment-resume）」。

## Phase 1（完了 2026-10-04、単一 phase）

本番 `1ba0cd48` の観測: task 01M3XSD0AYXTK2E7SC0JMX9M02 の run 01M42CD139F0NAGBJ5EF0FEXJP が `interrupted: comment` で
終わり、次の run 01M42CD7GMR46VQ4MYSJKYSMSK は判断表 #3 で `fresh (reason=not_continuation)`。

### 入れたもの

- `crates/task-dispatch/src/sessions.rs`: `ContinuationFacts.previous_comment_interrupt`。判断表 #3 はこれが真なら
  continuable（ほかの行はそのまま）。
- `crates/task-dispatch/src/dispatcher/continuation_session.rs`:
  - `interrupted_by_comment`: 直前の run の `WorkerFinished` が `interrupted: comment`、または `interrupted: …` で
    直前の `Transitioned` が `reason=comment, to=ready`（v2 の工程 lease の WU の run は abort で閉じられるため）。
  - `without_resume_rejected_finishes`: resume を拒否された run の `WorkerFinished` を割り込みの消化に数えない。
- `crates/task-dispatch/src/dispatcher/run_context.rs`: 上を `interrupting_comment` に渡す（session 欠落の fallback run にも
  コメント本文が載る）。
- `crates/task-dispatch/src/dispatcher/leases.rs`: `abort_stale_runs` が v1（段の無い計画）の WU の run も
  `reconcile_work_unit_run` で戻す（従来は WU が `running` のまま残り、コメント後に task が再 dispatch されなかった）。
- 試験（偽アダプタ、in-memory store、外部ネットワーク・CPU 負荷なし）:
  - `sessions::tests::session_resume_comment_interrupt_is_continuable`（印なし = `not_continuation`、印あり = resume、
    account・session 欠落・resume 拒否・container・cwd・fresh 要求・planner・rollover・adapter 変更はそのまま fresh）
  - `dispatcher::tests::session_resume::session_resume_comment_interrupt_resumes_same_session`（atomic）
  - `…_resumes_work_unit_session`（v1 WU）、`…_resumes_phase_work_unit_session`（v2 WU）
  - `…_missing_session_falls_back_to_checkpoint`（jsonl を消してから割り込み → resume 拒否 → `resume_rejected` の
    checkpoint 前置き fresh、コメント本文も載る）

### 証拠

- 修正前（試験だけ先に入れて実行）: `cargo test -p task-dispatch --lib session_resume_comment` → 3 failed。
  `session_resume_comment_interrupt_resumes_same_session` は「the run after a comment interrupt resumes」で失敗
  （次の run が fresh = `not_continuation`）。missing-session は run 数 3（拒否も fallback も起きない）、v1 WU は
  WU が `running` のまま止まり idle にならない。
- 修正後: `cargo test -p task-dispatch --lib comment_interrupt` → 6 passed, 0 failed。
- `cargo fmt --all` → 差分は自分の変更の整形のみ。
- `cargo clippy --workspace -- -D warnings` → exit 0（warning なし）。
- `cargo test --workspace` → exit 0、140 test binary、3870 passed / 0 failed / 13 ignored。

### 未解決事項

- 本番昇格は未実施（人が行う）。昇格後、コメントで止めた run の次の run の進行に
  `continuation session: resumed (session=…)` が出ることで確認できる。
- コメント中断の run は checkpoint を残さない（`saves_checkpoint` は予算切れ・yield・wait だけ）。session 欠落の fallback
  は直近の checkpoint（無ければ checkpoint なし）で始まる。従来と同じ。

### 提案

- なし。
