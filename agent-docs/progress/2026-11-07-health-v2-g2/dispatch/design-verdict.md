---
title: 健全性v2 G2 dispatch design-verdict — 設計 review の結果で計画を採用・replan・人に回す（events だけで待つ）
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch design-verdict — 設計 review の結果で計画を採用・replan・人に回す（events だけで待つ）

完了日: 2026-11-08

WorkUnit `design-verdict`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D3.5（3〜5）と I9。前の unit は [design-gate](design-gate.md)。

## 変更
件名「G2 design-verdict: 設計 review の結果で計画を採用・replan・人に回す（events だけで待つ）」の commit と、この進捗の commit。
- `crates/task-dispatch/src/health_review/design_verdict.rs`（`dispatcher::design_gate` の子 module。`design_gate.rs` から `#[path = "design_verdict.rs"]` で読む）:
  - `settle_design_review`: 設計 review の run の終わり（`on_design_review_finished` が `WorkerFinished`・quota・runs 索引を書いた後）に呼ぶ。待っている設計 review（`DesignReviewRequested` があり同じ `plan_sha256` の `DesignReviewed` が無い）が無ければ何もしない。run が `done` で終わらなければ判定を書かない（下の「やり直し」）。`done` なら task の `artifacts/review.json` の `design`（`DesignVerdictOut`）を読む。無い・読めない・`design` が無いときは不合格。
  - `record_design_verdict`: `Event::DesignReviewed { plan_sha256, review_run, pass, findings }` を追記する。合格なら `DesignReviewRequested` の計画を採用（下の `plan_adoption`）。不合格なら planner の再試行の文（`planner_retry_message("design review failed: <kind>(<severity>) <path>: <reason>; …")`）を `WorkerProgress` に残し、拒否した `execution-plan.json` を Phase F5-fix3 と同じく `execution-plan.rejected.*` に退ける。この印は既存の `planner_retry_pending`（計画のある task は `RunPlanner { replan: true }`）・`planner_rejections_since_last_plan`（`ExecutionPlannerContext.previous_attempt_errors`）・`replan_trigger_reason`（新しい arm: `DesignReviewed { pass: false }` の findings を `replan_reason` にする）で planner の prior になる。新しい欄は足していない。同じ task で不合格が 2 回続いたら（`consecutive_design_review_failures`: 最後の `ExecutionPlanned` か合格より後の `pass: false` の数）、その計画を採用したうえで `Trigger::PlanGate` に遷移し `PlanApprovalRequested.reasons` に `design_review_failed:<n>` を入れる（人は承認か replan を選ぶ）。
  - `adopt_reviewed_plan`: event の計画を `validate_with`（上限だけの違反は /3 の緩めた上限で再検証。planner の最後の試行と同じ）→ `prepare_adoption` → `adopt_prepared` → `plan_approval_of`。task は `ready` なので遷移は `PlanGate` だけで、採用は events の追記だけ（`Trigger::Continue` は `running` からしか引けない）。採用に失敗したら planner の再試行の文を残す。
- `crates/task-dispatch/src/health_review/plan_adoption.rs`（`dispatcher::planner_flow` の子 module。`planner_flow.rs` から `#[path = "../health_review/plan_adoption.rs"]` で読む）: `on_planner_finished` の採用の手順（unit の gate `tree_plan_gate`、子 task の検証 `plan_children`・`tree_plan_checks`、`replan` / `adopt_plan_with_children`、木の止め `apply_tree_plan_holds`、root の承認 `root_plan_approval`、承認の要らない計画の報告）を `prepare_adoption`・`adopt_prepared`・`plan_approval_of`・`record_plan_adopted_notice` に移し、planner run の計画と設計 review を通った計画で共有した（二重実装にしない）。遷移は呼び出し側が task の状態に合わせて選ぶ。
- `crates/task-dispatch/src/health_review/design.rs`（純粋な部分）: `design_review_started` を `design_review_runs`・`design_review_in_flight`（`WorkerStarted { role: reviewer }` に同じ run の `WorkerFinished` が無い間だけ in flight）に替えた。`consecutive_design_review_failures`、`planner_run_before`（`DesignReviewRequested` を出した planner run）、`design_findings_line`、`design_review_failed_reason` を足した。
- `crates/task-dispatch/src/health_review/design_gate.rs`: `design_review_hold` は in flight の run があれば待ち、無ければ起こし直す。起こした run の数が `[review] max_reviewer_retries` を超えたら不合格として記録する（`done` で終わらない run・失われた run の上限）。`holds_design_review`（下）。`on_design_review_finished` の最後で `settle_design_review` を呼ぶ。
- `crates/task-dispatch/src/dispatcher/leases.rs`: `abort_stale_runs` は lease を持たない run でも、この task の待っている設計 review の run（`holds_design_review`）なら止めない。design-gate では設計 review の run を `running` に task の run として入れていたが、task は `ready` で lease が無いので次の tick の `abort_stale_runs` が `interrupted: aborted (task no longer running under this lease)` で止めていた（design-gate の試験は adapter が同じ tick で終わるので見えなかった。この unit の再起動の試験で見つけた）。
- `crates/task-dispatch/src/dispatcher/planner_flow.rs`: `on_planner_finished` の採用の約 200 行を `plan_adoption` の呼び出し（約 100 行）に置き換え。`replan_trigger_reason` に `DesignReviewed { pass: false }` の arm。
- 試験（prefix `health_review_design_verdict_`、6 件）:
  - `crates/task-dispatch/src/dispatcher/tests/health_review_design_verdict.rs`（5 件。一時 git repo に stub の検査器を commit、設計 review の run は台本どおり `review.json` を書く adapter）: 合格で `DesignReviewRequested` の計画が採用され unit の run が起きる（reviewer run は 1 本、PlanGate 無し）。不合格で planner が起き直し `previous_attempt_errors` に findings の種別・severity・path・理由が載り、2 回目の低 risk の計画が採用される。2 回続く不合格で 2 回目の計画が採用され `awaiting_plan_approval`（`PlanApprovalRequested.reasons` に `design_review_failed:2`）で unit は起きない。`review.json` に `design` が無ければ不合格（findings 空）で planner が起き直す。reviewer run の途中でデーモンが落ちても（`crash_for_test`）、新しい active（孤児の回収あり）は events から待ちを戻し、孤児の回収が失われた run を `WorkerFinished{interrupted}` で閉じた後に reviewer run を起こし直し、その合格で採用する（planner は起き直さない）。
  - `crates/task-dispatch/src/health_review/design/tests.rs`（1 件）: 続いた不合格の数え方と replan の理由の 1 行。
  - design-gate の試験 2 件（高 risk・方針を読めない）を、判定が無い reviewer は不合格で planner が起き直す、という verdict 後の挙動に合わせた。

## size・依存の制約
- `check-health.sh` は verdict=pass・new=0・worsened=0（existing 718）。`dispatcher/` の module の行は `on_planner_finished` の採用の手順を `plan_adoption.rs` に移した分で減り（`on_planner_finished` 自身も約 100 行減）、配線で足した行（`leases.rs` 3 行、`planner_flow.rs` の arm と `#[path]`）を相殺した。`dispatcher.rs` は触っていない。
- `deps.cycle`: 新しい file で `crate::dispatcher::…` と書くと `health_review` → `dispatcher` の辺ができ、base の循環（`dispatcher`・`review`・`undeclared_artifacts`）に `health_review` が加わって worsened になった。新しい file は `super::super::*` と `pub(in super::super)` で書き、`planner_flow.rs` からは `design_gate::design_verdict::design_review_failed_reason` と module の path で読む（検査器は `crate::<unit>` と外部 crate の path だけを辺にする）。

## 判断（ADR・前の unit との違い）
- 「不合格なら planner を replan で起こす」は、既存の planner の再試行の印（`WorkerProgress` の `もう一度だけ試します`）で表した。計画のある task は `planner_retry_pending` → `RunPlanner { replan: true }`、計画の無い task は compound の gate から planner が起きる。planner の試行の窓（`planner_attempts_in_window`、上限 2）は設計 review の不合格の後の試行も数えるので、不合格 → 再試行の計画が不正、なら従来の `plan_invalid` / atomic の経路に落ちる。
- 2 回続く不合格の `PlanGate` は、その計画を採用してから引く（`Trigger::PlanGate` は採用した計画の id を要る）。人が承認すればその計画で進み、replan を選べば note 付きで planner が起きる（既存の plan gate）。
- `done` で終わらない設計 review の run（adapter の失敗・中断・質問・予算切れ）は不合格に数えず、`[review] max_reviewer_retries` までもう 1 本起こす（final review のインフラ失敗と同じ上限）。超えたら不合格として `DesignReviewed { pass: false, findings: [] }` を書く。
- 再起動で失われた run は自分では閉じない。孤児の回収（`reconcile_ownerless_runs`。他に生きている instance が無ければ即、あれば期限まで待つ）が `WorkerFinished{interrupted}` を書いた後に、events だけの判定（in flight = `WorkerStarted` に `WorkerFinished` が無い）で起こし直す。draining の旧 daemon が持つ生きた run を新 daemon が止めないため。

## 証拠（提出時の HEAD で実行）
この WU では main の取り込み・rebase をしていない。下の検査は code の commit（件名「G2 design-verdict: 設計 review の結果で計画を採用・replan・人に回す（events だけで待つ）」）の後に実行した（後の commit はこの進捗の file だけ）。
- `bash scripts/dev/test-affected.sh` → exit 0（7 分 45 秒）。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。check-health は verdict=pass・new=0・worsened=0（existing 718）、repo_health_gate の試験は全部通過、nextest は 3546 passed・11 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p task-dispatch -E 'test(health_review_design_)'` → exit 0、14 passed（`health_review_design_verdict_` 6 件、`health_review_design_gate_` 8 件）
- `cargo nextest run -p task-dispatch -E 'test(health_review_) | test(planner) | test(plan_invalid) | test(replan) | test(v3_) | test(tree) | test(ownerless) | test(orphan) | test(lease) | test(abort)'` → exit 0、246 passed
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は `crates/task-dispatch/` の 10 file とこの進捗だけ（範囲外なし）
- 受け入れ条件 0（合格で採用・不合格で replan）: `health_review_design_verdict_pass_adopts_the_reviewed_plan`・`health_review_design_verdict_fail_replans_with_the_findings_as_prior`。条件 1（2 回続く不合格で PlanGate）: `health_review_design_verdict_two_failures_in_a_row_ask_a_human_plan_gate`。条件 2（待ちは events だけ・再起動で戻る）: `health_review_design_verdict_pending_review_survives_a_daemon_restart`・`health_review_design_gate_pending_review_is_read_from_events`。

## 未解決（後続の unit へ）
- `cargo clippy -p task-dispatch --tests` は base からある `dispatcher/tests/cos_slot_wait.rs:172` の `unnecessary_get_then_check` で落ちる（この unit の範囲外。gate の `cargo clippy --workspace -- -D warnings` は通る）。
- reviewer の prompt はまだ `ReviewRequest.design` を描かない（prompts task の review-health）。本番の reviewer が `design` を書くまで、高 risk の計画は不合格 → planner 再試行 → 2 回目の不合格で PlanGate になる。
- 設計 review の待ち・不合格を人に見せる表示（task の待ちの理由）は無い（`WorkerProgress` と `DesignReviewed` の event だけ）。
- 設計 review の run には task 木の token 予算の watch を付けていない（design-gate から持ち越し）。

## 提案
- 設計 review の不合格の findings を web の task 画面（計画の節）に出す unit を、G2 の後に置く。
