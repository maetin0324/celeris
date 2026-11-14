---
title: "健全性v2 R4 settle-root: run_settlement.rs の根で execution_scheduler の入力の型を再公開し、worker_finish.rs の直接の名指しを消す"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 settle-root

完了日: 2026-11-14

## 概要

- task の最終 review で受け入れ条件 0 だけが落ちた。落ちた式は `grep -q 'execution_scheduler::' $s/run_settlement.rs`。
- 原因: run_settlement が `execution_scheduler` の関数（`decide`・`no_progress_question`・`continuation_cap_question`・`settle_phase`）を
  呼ぶ箇所は下位の file `run_settlement/work_unit.rs`・`run_settlement/task_run.rs` にあり、module の根の file `run_settlement.rs` の
  コードは `execution_scheduler` を参照していなかった（根の module doc の `execution_scheduler` は `::` を含まない）。コードの不具合ではない。
- 直した内容:
  - `crates/task-dispatch/src/run_settlement.rs` の根で `pub(crate) use crate::execution_scheduler::ContinuationInputs;` として、WU の run の
    continuation の入力の型を再公開した。型は写さず、同じ型の再公開である。
  - `crates/task-dispatch/src/dispatcher/worker_finish.rs` の `settle_work_unit` の入力は `run_settlement::ContinuationInputs` で作る。
    `worker_finish.rs` には `execution_scheduler::` の字面が 1 つも無い（WU の run の入口は run_settlement だけ）。
  - `run_settlement/work_unit.rs` の import（`use crate::execution_scheduler::{ContinuationInputs, WuDecision, WuLimits};`）は変えていない。
- 振る舞いは変わらないので試験は足していない。既存の試験を同じ assertion で通した。ADR 2026-11-11-dispatcher-state-ownership と cex_ の試験は変えていない。

## 実装前 brief

1. 根本原因: 受け入れ条件 0 は根の file `run_settlement.rs` に `execution_scheduler::` の参照を求めるが、参照は下位の file にだけあった。
   症状は task の最終 review の受け入れ条件 0 の exit 1。
2. 責務 owner: `run_settlement` の module の根（WU の run の入口を出す場所）と、その入力を作る `worker_finish.rs`。
3. 既存の再利用先: `execution_scheduler::ContinuationInputs`（型の正本）。根の `pub(crate) use work_unit::{…}` と同じ再公開の形。
4. 選択肢: (a) 根で入力の型を再公開し、`worker_finish.rs` はその名前を使う（採用。型を写さず、WU の run の入口が run_settlement だけになる）。
   (b) 根のコメントに `execution_scheduler::` の字面を書く（check を字面で通すだけで、意味が無いので採らない）。
   (c) 判断の関数を根へ戻す（下位の file の分け方を崩し、根の行が増えるので採らない）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `worker_finish.rs` の `crate::execution_scheduler::ContinuationInputs` の直接の名指し。
7. 不変条件: run_settlement は永続化・file・process・Dispatcher に触れない。WU の遷移の判断は `execution_scheduler` の 1 か所にある。
   確かめる試験と検査は、下の受け入れ条件 0 の式と `cex_`・`structure_guard_` の試験。

## 変更（件名）

- `phase fix: run_settlement.rs の根で ContinuationInputs を再公開し、worker_finish.rs の execution_scheduler の名指しを消す`

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- task の受け入れ条件 0 の command（字面は同じ）: exit 0。

  ```sh
  s=crates/task-dispatch/src; test -f $s/run_settlement.rs && ! grep -rnE 'TaskStore|self\.store|\bstore\.|std::fs|tokio::fs|Command::new|\.spawn\(|(impl|&mut|&) *Dispatcher\b' $s/run_settlement* && grep -q 'execution_scheduler::' $s/run_settlement.rs && ! grep -nE 'struct (WuLimits|WuDecision)\b|fn (decide|no_progress_question|continuation_cap_question)\b' $s/run_settlement.rs && ! grep -nE 'execution_scheduler::(decide|no_progress_question|continuation_cap_question)\b' $s/dispatcher/worker_finish.rs
  ```

- `grep -qE '^(pub\(crate\) )?use crate::execution_scheduler::' crates/task-dispatch/src/run_settlement.rs`: exit 0。
  `grep -n 'execution_scheduler::' crates/task-dispatch/src/dispatcher/worker_finish.rs`: exit 1（0 件）。
- `cargo nextest run -p task-dispatch -E 'test(cex_) | test(structure_guard_)'`: 36 passed、1809 skipped。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3900 passed、11 skipped。repo_health_gate は all checks passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（verdict=pass）。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`: exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs agent-docs/adr/2026-11-11-dispatcher-state-ownership.md`: exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 path は `run_settlement.rs`・`worker_finish.rs`・この unit の進捗 2 件で、範囲外 0。

## 未解決事項

- 無し。

## 提案

- 無し。
