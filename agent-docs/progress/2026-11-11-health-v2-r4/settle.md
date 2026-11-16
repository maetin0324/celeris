---
title: "健全性v2 R4 settle: run の終わりの判断を run_settlement へ分け、等価な閉じ方を共有する"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 settle

完了日: 2026-11-14

## 結論

- `finish_worker_result_with`（`crates/task-dispatch/src/dispatcher/worker_finish.rs`）の run の判断を、純粋な module
  `crates/task-dispatch/src/run_settlement.rs`（と `run_settlement/work_unit.rs`・`run_settlement/task_run.rs`）へ移した。
  `worker_finish.rs` には入力の読み出しと提案の適用が残る。`finish_worker_result_with` は 772 行（settle-task の check の awk で数えた値）。
- WU の遷移の判断の正本は `execution_scheduler` のまま。`run_settlement/work_unit.rs` が `crate::execution_scheduler::decide(` を呼び、
  `WuLimits`・`WuDecision`・`decide`・`no_progress_question`・`continuation_cap_question` を写していない。`worker_finish.rs` は `decide` を呼ばない。
- ADR 2026-11-11-dispatcher-state-ownership の D4 で担当 unit が settle の 3 関数は、表の module に 1 つずつ定義され、2 か所以上から呼ばれる。
  - `run_settlement::replan_or_ask`（wu-failure）: 呼び出し 5 か所（`phase_integration.rs`・`tree_units.rs`・`work_units.rs` 2・`run_settlement/work_unit.rs`）。
  - `run_settlement::run_end_record`（run-close）: 呼び出し 10 か所（`leases.rs` 3・`planner_flow.rs` 2・`worker_finish.rs` 2・`review_verdict.rs`・
    `health_review/design_gate.rs`・`run_supervisor.rs`）。閉じる経路 `close_aborted_run`・`close_run_after_finalisation_failure`・
    `reconcile_ownerless_runs` は別々の関数のまま残り、lease の検証・状態遷移・transaction の境界は経路ごとにある。
  - `sessions::stored_session_fresh_reason`（continuation-resume）: 呼び出し 2 か所（`sessions.rs` の `decide_continuation`、
    `sessions/cos_chat.rs` の `decide_cos_chat_session`）。
- 人の決定 fresh-contract の答えは **a**（共有を「保存 session の id の妥当性・context の枯渇・token の上限」に絞る。照合 key と前回の拒否は
  各関数に残す。ADR 2026-11-11 の本文は変えない）。絞り込みは
  [ADR 2026-11-13-continuation-resume-shared-inputs](../../adr/2026-11-13-continuation-resume-shared-inputs.md) に書いた。D4 の
  continuation-resume の契約を id・context・token の照合に絞り、表の「拒否」と契約の入力「照合 key の一致」「前回の拒否」は共有しない（同 ADR D4 が正本の付記）。
- ADR 2026-11-11-dispatcher-state-ownership と cex_ の試験は変えていない。transaction の範囲は変えていない（D3）。
- task の最終 review で受け入れ条件 0 が落ちた（根の `run_settlement.rs` に `execution_scheduler::` の参照が無かった）。settle-root が根で
  `pub(crate) use crate::execution_scheduler::ContinuationInputs;` と再公開し、`worker_finish.rs` は `run_settlement::ContinuationInputs` で
  入力を作るようにした。`worker_finish.rs` に `execution_scheduler::` の字面は無い（[settle-root.md](settle-root.md)）。

## 各 unit の要点

| unit | 要点 | 進捗 |
|---|---|---|
| settle-replan | `run_settlement` を作り、wu-failure の上限判定（`replans_so_far` と `effective_max_replans` の比較、stalled の比較）を `replan_or_ask` の 1 つにした。入力は `ReplanLimits::from_events`、読み出しは `tree_units.rs` の `replan_limits`。`Dispatcher::counted_replans` を消した | [settle-replan.md](settle-replan.md) |
| settle-endrec | 終了記録（`Event::WorkerFinished`）の組み立てを `run_end_record` の 1 つにした。追記・lease の検証・状態遷移は各経路に残した | [settle-endrec.md](settle-endrec.md) |
| settle-fresh | 決定 fresh-contract の答え a に従い ADR 2026-11-13 を先に足し、`sessions::stored_session_fresh_reason`（id → context → token）を 1 つだけ定義した | [settle-fresh.md](settle-fresh.md) |
| settle-end | `run_settlement::settle(RunFinishInput) -> RunFinishProposal` を作り、終わり方・`Trigger`・outcome の文・`RunEnd`・infra 再試行・requeue の上限・cluster job の wait・browser の前提・check の不合格を移した。`questions_of_run`・`stop_at_token_budget` は別の純粋関数 | [settle-end.md](settle-end.md) |
| settle-wu | `settle_work_unit` が `execution_scheduler::decide` を呼び、auto-leaf・merge の repair・integration repair・replan か質問・決定の保留を決める。`worker_finish.rs` から `decide` の直接の呼び出しを消した | [settle-wu.md](settle-wu.md) |
| settle-task | `settle_task_run`（atomic task の continuation・子待ち・進捗なし・上限）と `phase_step_after_write`（`settle_phase` を呼ぶ段の進み方）を足した。`worker_finish.rs` から `no_progress_question`・`continuation_cap_question`・`settle_phase` の参照を消した | [settle-task.md](settle-task.md) |

## 変更（件名）

- settle-replan: `phase settle: run_settlement::replan_or_ask で wu-failure の Replan か Ask の上限判定を 1 つにする`、
  `phase settle: replan_or_ask の入力を replan_limits で読み、呼び元の行を増やさない`、
  `phase settle: replan_gate の doc comment に共有の判定を書き、行を増やさない`、
  `phase settle: replan_limits を counted_replans に置き換え、events からの組み立てを ReplanLimits::from_events に移して dispatcher の行と method を増やさない`
- settle-endrec: `phase settle: run の終了記録の組み立てを run_settlement::run_end_record の 1 つにし、閉じる経路は分けたまま残す`
- settle-fresh: `phase 1: settle-fresh: continuation-resume で共有する照合を id・context・token に絞る ADR を足す`、
  `phase 1: settle-fresh: stored_session_fresh_reason を sessions に 1 つだけ定義し、decide_continuation と decide_cos_chat_session から呼ぶ`
- settle-end: `phase settle: worker の run の終わり方の判断を run_settlement::settle に移し、部をまたぐ質問と木の token 予算切れの判断も run_settlement に置く`、
  `phase settle: settle を終わり方の分類と失敗の分類に分け、run_settlement から review と run_supervisor への依存をなくす`
- settle-wu: `phase settle: settle に WU の run の判断を足し、execution_scheduler::decide を run_settlement から呼ぶ`
- settle-task: `phase settle: settle に atomic task の continuation と段の進み方の判断を足し、worker_finish.rs を適用だけにする`
- settle-record（この unit）: この進捗ファイルだけ。コードと ADR は変えていない。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

統合後の branch（integrate wu/settle-task の後）の HEAD で実行した。

- 計画の check（純粋さと D4）: exit 0。
  - `run_settlement.rs`・`run_settlement/` に、check が grep する永続化・file・process・`Dispatcher` の字面は無い（grep exit 1）。名前に decide を含む関数も無い。
  - D4 の表で担当 unit が settle の 3 行を awk で引き、定義と呼び出しを数えた: `sessions::stored_session_fresh_reason` 定義 `sessions.rs`・呼び出し 2、
    `run_settlement::run_end_record` 定義 `run_settlement.rs`・呼び出し 10、`run_settlement::replan_or_ask` 定義 `run_settlement.rs`・呼び出し 5。
  - `worker_finish.rs`・`phase_integration.rs` に `replans_so_far < self.effective_max_replans` は無い。
  - `cargo nextest run -p task-dispatch -E 'test(cex_) | test(structure_guard_)'`: 36 passed、1809 skipped。
- main の `scripts/dev/repo-health/profiles/celeris.toml` の `[[review.high_risk]]` の lease と budget の正規表現は、どちらも
  `crates/task-dispatch/src/run_settlement.rs` に当たる（各 1 本）。
- `sh scripts/dev/check-doc-links.sh`: exit 0。`sh scripts/dev/check-adr-numbers.sh`: exit 0（437 files）。
  `sh scripts/dev/progress-index.sh --check`: exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ agent-docs/adr/2026-11-11-dispatcher-state-ownership.md`: exit 0（コードと ADR 2026-11-11 は不変）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 path はこの進捗ファイルの 1 件、範囲外 0。
- `bash scripts/dev/test-affected.sh`: exit 0（約 140 秒）。repo_health_gate は all checks passed、「試験対象の変更なし（docs のみ等）」。
  この unit はコードを変えていないので crate の試験は走らない。統合後のコードの試験は各 unit の進捗の test-affected（settle-task で 3897 passed）と、
  上の cex_・structure_guard_ の 36 passed に拠る。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決事項

- cex-lease.md の提案（WU の done・依存 WU の昇格・Task の遷移・runs の閉じを 1 つの原子更新にする案）は、settle では扱っていない。
  ADR 2026-11-11 D3 が「transaction の範囲は今と同じにする。今別々に commit するものを 1 つにまとめない」と決めているため。
  反例（WU a を done にした直後に落ちると、再起動後に b・c が pending のまま残り、Task が ready で止まる）は今も成り立つ。
  `cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` はこの前進を assert していない（[cex-lease.md](cex-lease.md)）。
- settle-replan が置き換えなかった箇所:
  - `planner_flow.rs` の `replans_left`・`node_replans_left`: 上限との比較でなく、prompt に渡す残り回数の引き算の表示なので `replan_or_ask` を使わない
    （入力は `replan_limits` から読む）。
  - replan の実行（`AutoReplanRequested`・`Continue{Replan}`）・event の追記・質問の文・決定の要求（`raise_node_replan_limit`）・`PLAN_ISSUE_REPLAN_HINT`:
    D4 の wu-failure の行で「共有しない」に当たるので各経路に残した。
- settle-endrec が置き換えなかった箇所:
  - `crates/task-core/src/store/events.rs` の `close_runs_of_terminal_tasks`（`WorkerFinished{end: Cancelled}`）: task-core は task-dispatch に依存できず、
    組み立ては store の transaction の中にある（store は高 risk の path class）。
  - `crates/task-ops/src/comment.rs` の人のコメントの割り込み（`interrupted: comment`）: task-ops は task-dispatch の下の crate で、`run_settlement` を呼べない。
  - 既にある `WorkerFinished` を match で読むだけの箇所（`auto_leaf.rs`・`child_tasks.rs` など）と試験の fixture: 組み立てでないので変えていない。
- settle-fresh が置き換えなかった箇所:
  - 前回の拒否と照合 key: task・WU と CoS chat で見る順が逆で、既存の試験 2 本（`session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint`・
    `cos_chat_run_session_reason_precedence_is_fixed`）が今の順を求める。拒否だけが checkpoint の文脈を前置きに入れる（ADR 2026-11-13 D2）。
  - cwd の不一致: task・WU では `SurfaceUnsupported`（session を作らない）、CoS chat では照合 key の一部で、意味が違う（ADR 2026-11-13 D2）。
  - node 単位の session の `sessions::decide`（ADR-0054 D1）: D4 の表に無く、順も id → account → token なので含めない（ADR 2026-11-13 D3）。
  - 拒否と照合 key の順を 1 つにそろえるかは、後で人が決める問いとして残る（ADR 2026-11-13「残る問い」）。
- ADR 2026-11-11 D4 の表の continuation-resume の行は「拒否」を共有すると書いたままである。正本の付記は ADR 2026-11-13 D4。

## 提案

- cex-lease の反例は、ADR D3 と合う「再起動の照合が、依存が全部 done の pending WU を昇格させる」案で別の task として直す。直したら
  cex-lease.md に書いた assert（`run_until_state(.., Status::Done)`）を試験に戻す。原子更新の案を採るなら、先に ADR D3 の付記が要る。
- `questions_of_run`・`stop_at_token_budget`・`phase_step_after_write` は、どれも WU と atomic task の分岐の後の trigger を見る。読み出しの位置が
  3 つで違うので今は分けた。1 つの「書き込みの後の段」の提案にまとめるかを後の unit で見る（settle-end・settle-task の提案）。
- `WorkUnitSettlement` の完了の要約（`run_index_get` を読む）と `RunMetrics` は `worker_finish.rs` に残した。前者は各 unit の `completed` を
  入力にすれば純粋関数にできる（settle-wu・settle-task の提案）。
