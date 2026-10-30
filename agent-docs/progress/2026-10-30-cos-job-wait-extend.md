---
title: クラスタ job の待ち上限の認可に、案件 CoS が自動で「延長」と答える
tasks: [01M655XC0MZBH3CB9TDWHW1Y20]
status: done
updated: 2026-10-30
---
# クラスタ job の待ち上限の認可に、案件 CoS が自動で「延長」と答える

完了日: 2026-10-30

ADR: [2026-10-30-cos-job-wait-extend](../adr/2026-10-30-cos-job-wait-extend.md)
（[2026-10-28-cos-routine-delegated-answers](../adr/2026-10-28-cos-routine-delegated-answers.md) に付記 A4）

## 変更

- 設計: CoS の定型の代答の 7 種類目 `job_wait_extend`。答えは `once` ＋ note「延長」だけ。API が決定的に検査する条件は
  (1) 待ち上限の到達で出た認可（events の `ClusterJobWaitFinished{timed_out}` の直後の `QuestionRaised` と approval の question が一致）、
  (2) 最後の poll で job が全て queued、(3) 答えが延長だけ、(4) human_required は既存の 403、(5) 同じ job の上限到達が 3 回まで。
- `crates/task-api/src/cos/basis/job_wait_extend.rs`（新規）: 上の検査と単体試験 3 件。
- `crates/task-api/src/cos/basis.rs`: `BasisCategory::JobWaitExtend`、`BasisTarget.approval_id`、problem code
  `cos_basis_job_not_queued`・`cos_basis_extend_limit`。
- `crates/task-api/src/cos/inbox.rs`: 受信箱の `authorization-<id>` と `/cos/operations` の `approval.decide` から approval id を渡す。
  `approval.decide` の自由文は body の `answer` なので、それを note として検査に渡す。
- `cos-inbox-triage` skill を version 9 に（§2 の表に待ち上限の行、§3 に (g) と (g) にしない条件）。`[cos.triage] policy_version` の既定 "9"。
  `config/celeris.example.toml`・`docs/ops/cos-chat.md`・`docs/api/v1/api.md`・`docs/ops/mlsys-main-4-ops-r1.md` §8 を更新。API schema と web の生成型を再生成。
- 試験: `crates/task-api/tests/cos_job_wait_extend.rs`（新規 3 件、fake harness の CoS run credential、LLM なし）。

## 証拠（提出時の HEAD で実行）

基準は `git merge-base HEAD main`。

- `cargo test -p task-api --test cos_job_wait_extend`: 3 passed。
- `cargo test -p task-api --lib job_wait_extend`: 3 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker の nextest 4776 passed / 20 skipped、web の tsc -b と vitest 697 passed / 101 files、node --test）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 本番への移行

- 本番 config に `[cos.triage] policy_version` を書いていれば "9" に書き換える（無ければ既定の "9"）。KB の skill 正本を使っていれば版 9 に差し替える。

## 未解決事項

- plan gate の自動承認（task 01M655SMDYDX81M6QF2N4YP5C1、`plan_conforms`）が先に main に入り版 "8" と (f) を使った。review 前の main 同期で衝突を解き、本 task は版 "9" と (g) にした（ADR D4）。

## 提案

- 延長の後の run が同じ job を待ち直すことは worker の指示（ADR-0090 の質問文）に頼っている。答えが「延長」のときに daemon が
  同じ job の wait を決定的に作り直せば、run を 1 回省ける。

## 修正（WU fix-source、2026-10-30）

review の差し戻し（rebase の衝突解消で SOURCE.md の変更履歴が崩れた）を直した。

- `config/skills/cos-inbox-triage/SOURCE.md`: plan_conforms（ADR 2026-10-30-cos-plan-conforms-approve）の項の末尾に
  「policy_version を 8 に上げた。既定も "8"」を戻した。job_wait_extend（ADR 2026-10-30-cos-job-wait-extend）の項の末行を
  「policy_version を 9 に上げた。既定も "9"」に直した。
- `crates/task-api/tests/cos_job_wait_extend.rs`: ingest の `policy_version` を "8" から "9" に揃えた。
- commit: 「phase 1: SOURCE.md の変更履歴の版を直し、job_wait_extend 試験の policy_version を 9 に揃える」。

### 証拠（提出時の HEAD で実行、基準は `git merge-base HEAD main`）

- `cargo test -p task-api --test cos_job_wait_extend`: 3 passed。
