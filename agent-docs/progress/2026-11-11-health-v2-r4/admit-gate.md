---
title: "健全性v2 R4 admit-gate: dispatch_one の前半（capacity から browser の前提まで）の順序と組み合わせを dispatch_admission::admit へ移す"
tasks: [01M7BYQNFCE4Z8PFD95T6VQ37J]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 admit-gate

完了日: 2026-11-14

## 概要

- [ADR 2026-11-14 dispatch-admission-order](../../adr/2026-11-14-dispatch-admission-order.md) を書き、
  `crates/task-dispatch/src/dispatch_admission.rs`（`admit`・`AdmissionInput`・`AdmissionProposal`）と
  `crates/task-dispatch/src/dispatch_admission/tests.rs`（試験 8 本）を足した。lib.rs で `pub(crate) mod dispatch_admission`。
- `dispatch_one` の前半（知識整理の入力から browser の前提まで）は、admit が求めた fact を既存の関数で集める loop になった。
  止め方（待ちの記録・統合の開始・最終 review の開始）と起動（予算の上書き）は admit の提案を dispatch_one が適用する。
- `dispatch_run.rs` に `second_pass` の条件式と `shadow_human_explicit_compound`・`tree_child_compound` は無い。
  `second_pass` は WU の worktree の用意の失敗と task の lease の引数（admit-place・admit-launch の範囲）にだけ残る。
- `dispatch_one` の行数: 移行前 1089 行（30〜1118 行）→ 移行後 1023 行（34〜1056 行）。`dispatch_run.rs` は 1704 行 → 1676 行。
- ADR 2026-11-11 と `cex_` の試験と既存の試験は変えていない。

## 実装前 brief

1. 根本原因: `dispatch_one` は gate を見る順序・並列 WU の 2 本目以降の飛ばし方・planner run の組み合わせと、待ちの記録・遷移・
   担当の書き戻しなどの副作用を 1 つの関数で交互に行う。順序の入れ替えを検出する試験が無い（既存の試験は 1 つの gate だけを止める）。
2. 責務 owner: 判断は新しい純粋 module `dispatch_admission`（ADR 2026-11-11 D3・D8 が名指し）。副作用は `dispatch_one` と、
   それが呼ぶ owner の method（`RunSupervisor`・`ExecutionCoordination`・`HostMaintenance`）。
3. 既存の再利用先: 個々の判定は既存の関数をそのまま呼ぶ。`task_ops::knowledge_curation::inputs_ready`・`runs_for_task`・
   `is_cos_task`・`RunLoad::admits`・`was_just_aborted`・`is_eligible`・`held_by_disk_critical`・`disk_watch_paths`・
   `assign_if_needed`・`execution_gate_if_needed`・`apply_plan_invalid_atomic`・`wu_dispatch_gate`・`decision_self_hold`・
   `design_review_hold`・`tree_units::next_run`・`tree_limit_hold_for`・`task_core::tree::is_tree_child`・`retry_backoff`・
   `infra_backoff_until`・`clear_infra_backoff`・`browser_prereq_applies`・`browser_prereq_hold`。
4. 選択肢: (a) admit が次に集める fact を 1 つずつ求める loop（採用。今は呼ばれない副作用を起こさず、順序を admit だけが持つ）。
   (b) fact を全部先に集めて 1 回で判断する（前の gate が止めたら呼ばない副作用が起き、順序が変わるので不採用）。
   (c) admit が fact を集める trait を受けて呼ぶ（admit が副作用のある関数を呼ぶので純粋でなくなり不採用）。
5. 新しい状態・分岐・fallback・port: 状態と fallback は無し。新しい型は入出力の値（`AdmissionInput`・`AdmissionUnit`・
   `AdmissionTaskValues`・`BackoffClock`・`AdmissionProposal`・`AdmissionFact`・`AdmissionStop`・`AdmissionLaunch`）だけで、
   判定の型は足していない（ADR 2026-11-11 D5）。`AdmissionUnit` は dispatcher の非公開の型 `WuDispatchGate` を落とした値。
6. 撤去対象: `dispatch_one` の中の gate の順序の分岐（`second_pass` の条件式 8 か所）、planner run の組み合わせの変数
   （`gate_decision`・`decision_is_compound`・`shadow_human_explicit_compound`・`tree_child_compound`・`direct_route` の計算・
   `is_planner_dispatch` の式）、予算の上書きの計算、`replan_dispatch` の `mut` の書き換え。地図の行の
   「`dispatch_run.rs` の `is_planner_dispatch` で分岐」の記述も admit を指すように直した。
7. 不変条件: 権限と予算の判定を減らさず、順序を変えない。並列 WU の 2 本目以降が飛ばす gate の集合は変えない。
   確かめる試験は `dispatch_admission/tests.rs` の 8 本、`cex_` の試験、既存の dispatcher の試験（test-affected）。

## 順序の等価性で確かめたこと

- 移行前の `dispatch_one` と同じ順で fact を求め、同じ条件で後ろの関数を呼ばない（短絡）。決定の待ちが止めたら設計 review の待ちを
  求めない。`tree_units::next_run` が `None` の gate は `tree_limit_hold_for` を呼ばない。infra の backoff の解除は期限を過ぎたときだけ、
  browser の前提の前に行う。
- 担当が決まらないとき: 移行前は `assign_if_needed(task)` の `None` で `Ok(false)` を返した。移行後は task の写しを渡し、`None` なら
  admit が止め方（記録なし）を返す。担当の決定が blocked にした後の副作用は同じ。写しは担当を決める 1 本目の dispatch で 1 回だけ作る。
- 予算の上書きを task に当てる位置を、再試行と infra の backoff と browser の前提の後に移した。この 3 つの判定と止め方が読む task の値は
  id・attempts・更新時刻・種類・skills・requirements だけで、tier と予算を読まない（`browser_prereq_applies` は種類と skills、
  `browser_prereq_hold` は id と requirements）。止めたときは task を捨てるので、結果は同じ。
- backoff で止めたときの debug log の文を 1 つ（`backoff; not dispatching yet`、原因と期限つき）にした。待ちの記録（`TaskWait::Backoff`）は同じ。

## 変更（件名）

- `phase admit: dispatch_one の前半の起動可否の順序と組み合わせを dispatch_admission::admit へ移す`
- `phase admit: admit-gate の進捗に検査の結果を書く`（文書だけ。clippy・test-affected は前の commit と同じコードで走らせた）

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(cex_) | test(/^dispatch_admission::/) | test(structure_guard_)'`: exit 0、44 passed、1809 skipped。
- 順序の入れ替えを試験が検出すること（作業中に一時的に本体を書き換えて確かめ、元に戻した）:
  再試行と infra の backoff の順を入れ替えると 2 本が落ちた。決定と設計 review の待ちから「1 本目だけ」を外すと 1 本が落ちた。戻した後は 8 passed。
- WU の check 3 の grep の部分（純粋さ・同名の関数の定義なし・`admit`/`AdmissionInput`/`AdmissionProposal` が根の file・
  `dispatch_admission::admit(` の呼び出し・lease と spawn と `WorkerStarted` が `dispatch_run.rs` に残る・`second_pass` の条件式と
  planner の組み合わせの変数が無い・`#[test]` 8 本）: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（verdict=pass、fail_grade 0、worsened 0、new 1）。new の 1 件は warning の
  `migration.remnant`（`host_maintenance.rs` の `legacy` の語。この unit は触っていない）。size.responsibility-growth の悪化は無い
  （`Dispatcher` に method を足さず、落とす関数 `admission_unit`・`admission_task_values` は `dispatch_run.rs` の自由関数にした）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3908 passed、11 skipped。repo_health_gate は all checks passed。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- agent-docs/adr/2026-11-11-dispatcher-state-ownership.md <cex_ の試験の file>`: exit 0。
  `git diff --name-only "$CELERIS_WU_BASE" -- crates/task-dispatch/src/dispatcher/tests crates/task-dispatch/tests`: 0 件。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 path は ADR・この進捗・`dispatch_admission.rs`・`dispatch_admission/tests.rs`・
  `dispatch_run.rs`・`lib.rs`・`docs/architecture-map.md` の 7 件で、範囲外 0。

## 未解決事項

- 無し。

## 提案

- 無し。
