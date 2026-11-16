---
title: "健全性v2 R4 settle-end: settle を作り、task の run の終わり方・Trigger・outcome の文・retry の可否と token 予算切れの判断を移す"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-13
completed: 2026-11-13
---
# 健全性v2 R4 settle-end

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D3 の `run_settlement::settle(RunFinishInput) -> RunFinishProposal` を
  `crates/task-dispatch/src/run_settlement.rs` に作った。`finish_worker_result_with` は `crate::run_settlement::settle(` で呼ぶ。
- `settle` が決めるもの: 結果の分類（terminal・供給側の失敗・misplaced・部をまたぐ質問の抽出）、`Trigger`・outcome の文・`RunEnd`、
  infra 再試行（`InfraRequeue` か `infra failure ×N` で打ち切るか）、requeue の上限（認証失敗は数えない）、cluster job の wait、
  browser の前提の block（atomic の run だけ）、WU の check の不合格（usage の補いと outcome の要約）。中は `settle_terminal`（`Ok`）と
  `settle_adapter_failure`（`Err`）に分けた（1 関数 200 行の上限）。
- WU と atomic task の分岐の後に決まる trigger を見る 2 つの判断は、同じ module の別の純粋関数にした。
  - `questions_of_run`: 部をまたぐ質問が run の自己申告の終わり方より優先する置き換えと、人に見せる質問の列（旧 1255-1272 行）。
  - `needs_tree_tokens`・`stop_at_token_budget`（入力 `TreeTokens`、提案 `TokenBudgetStop`）: 木の token 予算切れ（旧 1422-1445 行）。
    木の消費は今と同じく、止める候補（`needs_tree_tokens`）のときだけ worker 完了の経路が読む。
- `describe_run_end`・`trigger_starts_another_run`・`classify_budget_kind_from_text`・`cross_department_questions_of`（`cross_department_questions`
  に改名）を run_settlement へ移し、`worker_finish.rs` の定義を消した。`dispatcher/token_budget.rs` の `outcome` は
  `run_settlement::token_budget_outcome` に移し、`review_spawn.rs` の呼び出しを直した（同じ文の写しを作らない）。
- `worker_finish.rs` が読むもの（入力に詰める）: events（1 回だけ読み、部をまたぐ質問と連続 requeue 数に使う。旧は同じ時点で最大 3 回読んでいた）、
  連続 infra 再試行数と `run_supervisor::infra_retry_after` の待ち時間、`provider_failure_outcome`、`browser_prereq::worker_error_code`、
  `cluster_job_wait_for` の検証の結果（`Terminal::Waiting` のときだけ）、`[execution]` の上限。
- 提案の適用は今と同じ順のまま `worker_finish.rs` と owner の method が行う（`RunSupervisor::start_infra_backoff`、
  `RoutingRuntime::settle_quota`、cooldown の記録、`apply_transition_with_events`）。transaction の範囲は変えていない（D3）。
- `run_settlement` は `review`・`run_supervisor` を使わない（`ReviewSubject` の代わりに done の要約と証拠を返し、`infra_retry_after` は
  worker 完了の経路が呼んで結果を渡す）。`run_supervisor` が `run_end_record` を使うので、逆向きの依存を足すと deps.cycle が落ちる。
- WU と atomic task の分岐（旧 607-1281 行）は動かしていない（settle-wu・settle-task の範囲）。
- `dispatcher.rs` の module map の run_settlement の行（同じ 1 行の中で）と `docs/architecture-map.md` の run_settlement の行を直した。ADR は変えていない。

## 実装前 brief

1. 根本原因: run の終わり方の判断が `finish_worker_result_with`（`Dispatcher` の method）の中で store の読み出しと混ざり、入力から提案を
   単体で試せない。
2. 責務 owner: `run_settlement`（D3 の表）。適用は `worker_finish.rs` と `RunSupervisor`・`RoutingRuntime`。
3. 既存の再利用先: `run_supervisor::infra_retry_after`（D4 retry-backoff）、`task_ops::derive::consecutive_requeues`、`run_end_record`。
4. 選択肢: (a) settle 1 つで token 予算も決める（木の消費を先に読む。読む回数と早期 return の経路が変わる）、(b) token 予算と部をまたぐ
   質問の置き換えを同じ module の別の純粋関数にする。(b) を採った。WU の分岐を動かさないこの unit では、分岐の後の trigger を見る判断を
   settle に入れられないため。
5. 新しい状態・分岐・fallback・port: 無し。`Terminal::Waiting` で検証の結果が無いときは「検証されていない wait」として retryable な失敗に
   する（呼ぶ経路は必ず検証の結果を渡す。到達しない）。
6. 撤去対象: `worker_finish.rs` の `describe_run_end`・`trigger_starts_another_run`・`classify_budget_kind_from_text`・
   `cross_department_questions_of` と分類の本体、`token_budget::outcome`、`dispatcher.rs` の `consecutive_requeues` の import。
7. 不変条件: 既存の worker 完了の試験が同じ assertion で通る（test-affected）。cex_ の試験を書き換えずに通る。settle の境界は下の試験。

## 足した試験（`crates/task-dispatch/src/run_settlement/tests.rs`）

守る不変条件と既存の試験で検出できない理由は各試験の doc comment に書いた。

- `settle_requeues_supply_failures_below_the_limit`: requeue の上限ちょうど・認証失敗の例外・quota と cooldown に渡す分類。
- `settle_retries_infra_failures_until_the_limit`: infra 再試行の (n, 待ち時間) と打ち切りの文。
- `settle_blocks_on_browser_prerequisites_only_for_atomic_runs`: WU の run では block しない。
- `settle_classifies_errors_and_check_failures`: 予算切れの語彙は retryable のときだけ、check の不合格の usage と要約。
- `questions_of_run_puts_cross_department_questions_first_in_priority`: 置き換えと、既に質問のときの質問の列。
- `stop_at_token_budget_stops_at_the_limit_including_this_run`: 上限ちょうどで止め、この run の usage を足す。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh`: exit 0（696 秒）。3885 tests run: 3885 passed、11 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0。verdict=pass、fail_grade=0、worsened=0、new=1。new の 1 件は `migration.remnant`（warning）で
  `crates/task-dispatch/src/host_maintenance.rs` の `legacy` の行（この unit は変えていない。settle-endrec の進捗と同じ）。
- `cargo nextest run -p task-dispatch -E 'test(run_settlement) | test(cex_) | test(structure_guard_)'`: exit 0、46 passed。
  cex_ の試験と ADR は変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- …cex_lease_restart.rs …cex_review_authority.rs …2026-11-11-dispatcher-state-ownership.md` は exit 0）。
- 計画の check（settle・RunFinishInput・RunFinishProposal・describe_run_end・trigger_starts_another_run の定義が run_settlement だけにあり、
  `worker_finish.rs` が `run_settlement::settle(` を呼ぶ）: exit 0。純粋さの grep: exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: exit 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- `dispatcher/` の行: `worker_finish.rs` は 326 行減・73 行増、`token_budget.rs` は 5 行減。`Dispatcher` の method は足していない。

## 未解決事項

- 無し。

## 提案

- settle-wu・settle-task が WU と atomic task の分岐を settle に入れた後、`questions_of_run` と `stop_at_token_budget` を settle の後段
  （D3 の「WU の書き込みの後のもう 1 回の純粋関数」）にまとめられるか見る。
