---
title: 戦略層 v1 子 6 review-launcher — 設計 review の reviewer run の起動・実行・終わりの記録を 1 つの起動口にまとめる（挙動は不変）
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: review-launcher
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# review-launcher（WorkUnit）

完了日: 2026-11-22。
設計 review の reviewer run の起動・実行・終わりの記録を、`crates/task-dispatch/src/health_review/design_gate.rs` の中の 1 つの起動口にまとめた。
設計 review の events・`request.json`・runs 索引・試行の数・木の上限の扱いは変えていない（base の code で取った sha256 で固定した試験で確かめた）。
従う設計は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D2.3・D11。ADR は書いていない（adr-revise が持つ）。
親の途中評価の種類（`SideReviewKind::ParentEvaluation`）は足していない（controller-checkpoint が足す）。
基準はこの WU の branch の起点（adr-revise の後の HEAD）と、check が使う `git merge-base HEAD main`。検査は提出時の HEAD で実行した。

## commit（件名）

1. `strategy-v1 6 review-launcher: 設計 review の reviewer run を共通の起動口 start_side_review・on_side_review_finished・holds_side_review にまとめ、試験 reviewer_side_run_ と WU の進捗`

## 実装前 brief

1. 根本原因: task の status を変えない reviewer run の起動・実行・終わりの記録が、設計 review 専用の関数（`start_design_review`・`run_design_reviewer`・`on_design_review_finished`）と
   専用の完了（`Completion::DesignReview(DesignReviewEnd)`）に閉じていた。親の途中評価が同じ手順を要るので、そのままでは 3 本目の写しになる（計画 v1 の設計 review の blocking）。
2. 責務 owner: `health_review/design_gate.rs`（`dispatcher` の子。dispatcher の private な `running`・`Completion` を使う）。`abort_stale_runs` の例外は `dispatcher/leases.rs` の呼び出し 1 行。
3. 既存の再利用先: `pick_reviewer`・`token_budget::run_watch`・`prompt_ancestors::lineage`・`record_route_selection`・`quota_begin`・`settle_quota`・`run_end_record`・`run_end_of`・
   `review::run_within_token_budget`・`design::planner_run_before`（対象の planner run の探索。前は同じ探索を起動口の中に書き写していた）。試験は `health_review_design_gate` の fixture（`checker_repo`・`stub`・`v3_plan_with`）。
4. 選択肢: (a) 起動口を新しい file `health_review/reviewer_run.rs` に置く → `dispatcher` の子としてしか読めず、repo health の単位は同じ `task-dispatch/src/dispatcher` で、file と `mod` の行が増えるだけ（ADR D11）。
   (b) `Completion::DesignReview` の名前のまま `kind` を持たせる → design_gate.rs に設計 review 専用の名前の variant が残る（受け入れ条件 3）。採用: (c) `design_gate.rs` の中で一般化し、variant を `Completion::SideReview` に改名して end の struct に `kind` を持たせる。
5. 新しい状態・分岐・fallback・port: 完了の種類の enum `SideReviewKind`（今は `Design` の 1 つ）と、終わりの振り分けの `match`。状態は持たない（種類は run の完了に載るだけで、DB にも events にも書かない）。fallback・切替の flag は無い。
6. 撤去対象: `DesignReviewEnd`・`Completion::DesignReview`・`start_design_review`・`run_design_reviewer`・`on_design_review_finished`・`holds_design_review`（名前は crates に残っていない）、
   起動口の中の planner run の探索の写し（14 行を `planner_run_before` の 1 行に）。
7. 不変条件と試験: 下の「守る不変条件」の表。

## 移した関数と消した写し

| 前（base） | 後 | 中身 |
|---|---|---|
| `start_design_review(task, DesignReviewRequest) -> Result<()>` | `start_side_review(task, task_worker::ReviewRequest, SideReviewKind) -> Result<Option<String>>` | `pick_reviewer`・`run_watch`・`lineage`・`WorkerStarted{role: reviewer}`・`run_index_start`・route の記録・`quota_begin`・`tokio::spawn`・`register_run`。起こした run の id を返し、task の dir が無い・reviewer を選べないときは `None` |
| `run_design_reviewer(…, DesignReviewRequest, …)` | `run_side_reviewer(…, ReviewRequest, label, …)` | 合成の `Review` task・`RunContext.review` に渡された節をそのまま入れる。progress の文は `"{label} started (adapter=…)"`（設計 review は `design review` で前と同じ文） |
| `on_design_review_finished(DesignReviewEnd)` | `on_side_review_finished(SideReviewEnd)` | `WorkerFinished`（role reviewer）・`settle_quota`・`run_index_finish` の後に `match kind { SideReviewKind::Design => settle_design_review }` |
| `holds_design_review(task, run_id)` | `holds_side_review(task, run_id)` | 今は設計 review の半分だけ（待っている `DesignReviewRequested` の、まだ終わっていない reviewer run）。評価の半分は controller-checkpoint が足す |
| `Completion::DesignReview(Box<DesignReviewEnd>)` | `Completion::SideReview(Box<SideReviewEnd>)` | `SideReviewEnd` に `kind: SideReviewKind` を足した。新しい channel は無い |
| 起動口の中の planner run の探索（14 行） | `design::planner_run_before(&events, u64::MAX)` | 同じ探索（最後の `WorkerStarted{role: planner}`）の既存の関数を使う |

`design_review_hold` は `ReviewRequest { design: Some(..), ..Default::default() }` を組んで `start_side_review(task, review, SideReviewKind::Design)` を呼ぶ。
`leases.rs` の差分は `holds_design_review` → `holds_side_review` の 1 行（`-` 1 行・`+` 1 行）。`dispatcher.rs` は variant と doc comment と受け口の 3 行。

## LoC（この WU の差分。`git diff --numstat HEAD`、提出時の作業ツリー）

| path | 追加 | 削除 | 本体の行（空行・注釈・use・mod を除く。base → 後） |
|---|---:|---:|---|
| `crates/task-dispatch/src/health_review/design_gate.rs` | 93 | 79 | 477 → 476（全行 571 → 585。増えたのは doc comment） |
| `crates/task-dispatch/src/dispatcher.rs` | 3 | 3 | 1407 → 1407（全行 2156 → 2156） |
| `crates/task-dispatch/src/dispatcher/leases.rs` | 1 | 1 | 605 → 605 |
| `crates/task-dispatch/src/health_review/design_verdict.rs` | 3 | 3 | 199 → 199（doc comment の variant の名前） |
| production の計 | 100 | 86 | 本体 −1 行 |
| `crates/task-dispatch/src/dispatcher/tests/reviewer_side_run.rs`（新） | 328 | 0 | 試験 |
| `crates/task-dispatch/src/dispatcher/tests/mod.rs` | 3 | 0 | `mod` の行と doc comment |
| test の計 | 331 | 0 | |

`run_index_start` を持つ file の数は `git merge-base HEAD main` の 15 → 15。repo health の `size.responsibility-growth`（module `task-dispatch/src/dispatcher` の本体の行の合計）と
`source-size`（`dispatcher.rs` の production_size）を悪化させない範囲に収めた（最初の版は doc comment で `dispatcher.rs` が 2012 → 2014、本体が +30 行で check-health が fail になったので詰めた）。

## 試験

足した試験（`crates/task-dispatch/src/dispatcher/tests/reviewer_side_run.rs`）:

| 試験 | 守る不変条件 | 変更前 |
|---|---|---|
| `reviewer_side_run_design_review_writes_the_same_events_and_request` | 起動口を経た設計 review が、`DesignReviewRequested` の後から `DesignReviewed` までの events（9 件）と、adapter が受けた `RunRequest`（`runs/<run_id>/request.json` に書かれる形）を前と同じに書く。`review` の節は `design` だけで summary・evidence・criteria は空 | base の code で pass（期待の sha256 は base の code で 2 回走らせて同じ値を取った回帰の固定） |
| `reviewer_side_run_completion_kind_routes_to_the_design_verdict` | `Completion::SideReview`（`SideReviewKind::Design`）が次の tick で `on_side_review_finished` に届き、`WorkerFinished`（role reviewer）の後に `settle_design_review` が `DesignReviewed` を書く。`ReviewVerdict` には流れない | base では compile しない（`Completion::SideReview` が無い） |
| `reviewer_side_run_holds_the_running_design_review` | 走っている設計 review の run は lease を持たない（task は `ready`）が `abort_stale_runs` で止まらない。別の run の id・`WorkerFinished` の後は真にならない | base では compile しない（`holds_side_review` が無い） |

既存の試験が見ないもの: `health_review_*` は採用・replan・人への gate の結果を見るが、events の並びと `request.json` の byte 列、完了の channel の振り分けの口、`abort_stale_runs` の例外を直接は見ない。
正規化: reviewer run の id・一時 dir の path・合成の `Review` task の id を置き換え、`at`・`ts`・`wall_ms`・`*_at` の欄を落とす。task の id と時刻は fixture で固定する。

既存の `health_review_*` の試験は変えていない（`health_review_design_gate` の fixture を `use` しただけ）。

## 検査（提出時の HEAD で実行）

```
$ cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/health_review_|reviewer_side_run_/)'
     Summary [  23.385s] 89 tests run: 89 passed, 1875 skipped        # exit 0（reviewer_side_run_ 3 件を含む）

$ ! grep -rq holds_design_review crates/task-dispatch/src && grep -rq holds_side_review crates/task-dispatch/src/dispatcher/leases.rs && …（WU の check の 2 本目）
run_index_start files 15 -> 15                                       # exit 0。grep -rn holds_design_review crates は 0 件

$ bash scripts/dev/test-affected.sh
repo-health: verdict=pass exit=0 reviewable=false findings=710 fail_grade=0 new=9 worsened=2 existing=699
     Summary [ 552.337s] 3990 tests run: 3989 passed (12 slow), 1 failed, 11 skipped   # exit 100
        FAIL [ 360.085s] celeris-campaign::dagbench_process recovery_notice_continuation_celeris_after_wait
        （stage `control_2_run_to_completion` stalled (child did not exit; insurance 240s)）

$ cargo nextest run -p celeris-campaign --no-fail-fast -E 'test(=recovery_notice_continuation_celeris_after_wait)'
     Summary [  21.701s] 1 test run: 1 passed, 989 skipped           # exit 0（単独の再実行。load average 6.8）

$ cargo clippy --workspace -- -D warnings                            # exit 0
$ cargo fmt --all -- --check                                         # exit 0
$ sh "$CELERIS_WU_SCOPE_PATHS" | grep -vE '<WU の範囲の正規表現>'      # 範囲外の path 0 件
```

test-affected の 1 件の失敗は、全体の並走の中で子 process が 240 秒以内に終わらなかったもので、単独では 21.7 秒で通る。この試験は設計 review の経路を通らない（`dagbench_process.rs` に design review の参照は無い）。負荷によるものと判断したが、原因は未確認。

## 守る不変条件

| 不変条件 | 実装 | 試験 |
|---|---|---|
| 設計 review の events・`request.json` は一般化の前と同じ | `start_side_review`・`run_side_reviewer`・`on_side_review_finished` | `reviewer_side_run_design_review_writes_the_same_events_and_request`、既存の `health_review_*` |
| 終わりは種類で振り分け、final review の判定に流れない | `on_side_review_finished` の `match kind` | `reviewer_side_run_completion_kind_routes_to_the_design_verdict` |
| 走っている設計 review の run は `abort_stale_runs` で止まらない | `holds_side_review` | `reviewer_side_run_holds_the_running_design_review` |
| 試行の数・木の上限の扱いは同じ | `design_review_hold`（変えていない） | 既存の `health_review_design_budget_*`・`health_review_design_verdict_*` |
| dispatcher に LLM を入れない・新しい表・event の種類・migration を足さない | 差分は task-dispatch の 4 file と試験だけ | WU の範囲 check |

## 未解決

- 範囲の外の注釈が旧い名前 `Completion::DesignReview`・`start_design_review`・`run_design_reviewer` を指している（コードの挙動には関係しない）:
  `crates/task-dispatch/src/review.rs` 1202 行、`crates/task-worker/src/protocol.rs` 74 行（`docs/protocol/worker-protocol.schema.json` 4637 行の description に写る）、
  `crates/task-dispatch/src/health_review/design/tests.rs` 208・263・264 行。この WU の範囲 check はこれらの path を許さないので直していない。
- ADR D11 の試験の prefix は `side_review_`、この WU の目的と check は `reviewer_side_run_`。check に合わせて `reviewer_side_run_` にした。ADR の「保証と実装の対応」の表の試験名 `side_review_holds_design_review_run` は
  `reviewer_side_run_holds_the_running_design_review` に当たる（`holds_design_review` の名前を crates に残さない条件があるため）。
- `start_side_review` の対象の run（`pick_reviewer` の subject）は種類によらず task の最後の planner run。途中評価（root）で別の run を対象にするかは controller-checkpoint が決める。

## 提案

- 上の旧い名前の注釈 5 か所は、integrate-unify か controller-checkpoint（`review.rs` の `mod` の行を書く）の範囲に `crates/task-dispatch/src/review\.rs$`・`crates/task-worker/src/protocol\.rs$`・
  `docs/protocol/worker-protocol\.schema\.json$`・`crates/task-dispatch/src/health_review/design/tests\.rs$` を足して直す。判断者は子 6 の planner（再計画）。
- final review（`review_spawn.rs` の 800〜880 行）の `WorkerStarted`・runs 索引の seq・`run_index_start`・route の記録・`quota_begin` は、起動口の同じ手順の写しのまま残っている（base から）。
  side review の起動口と final review の起動の共通化は、この子の範囲の外として親の closeout で人に提案する。
