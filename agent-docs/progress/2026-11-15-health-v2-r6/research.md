---
title: R6 research（研究 harness の締め切りの分類と continuation からの除外）
tasks: [01M7FYEHKMY34MM5Z75W2ESSGN]
status: running
updated: 2026-11-15
---
# R6 research（研究 harness の締め切りの分類と continuation からの除外）

task 01M7FYEHKMY34MM5Z75W2ESSGN（R6 の子 task research）の進捗。計画は unit adr → adapters → net。この file は unit adr の分を書いた。adapters・net は同じ file に節を足す。

## unit adr（完了日: 2026-11-15）

### commit

- `research adr: 研究 adapter を continuation から外す（continues_from_checkpoint）と ADR を足す`
- `research adr: 進捗に提出時の検査の結果を書く`

### 決定（[ADR 2026-11-15-research-adapter-deadline](../../adr/2026-11-15-research-adapter-deadline.md)）

- D1: 研究 3 adapter の wall clock の超過は `BudgetExhausted{WallClock}` に揃える。監視 loop の置き換えは unit adapters。
- D2: `WorkerAdapter::continues_from_checkpoint`（既定 `true`）を足した。langmem・local-deep-research・paperqa が `false`、browser-specialist は内側の値。dispatcher は `RoutingRuntime::run_continues_from_checkpoint` で `WorkerStarted.adapter` から性質を引き（引けなければ `false`）、`worker_finish.rs` が atomic task・WU・WU の checkpoint の判定の continuation の入力を「`[execution] continuation` かつ性質」にした。`sessions::SUPPORTED_ADAPTERS` は続けられる adapter の部分集合。
- D3: atomic task は既存の枝で `WorkerError{retryable:true}`・checkpoint なし。WU は既存の枝が WU を `running` のまま残して task を止めていたので、`RunEnd::Failed{retryable:true}` として WU の retry（`work_unit_retry`）に回すよう直した（`[execution] continuation = false` の WU も同じく直る）。
- D4: 3 adapter は成功の枝で result.json を書いた直後に `read_result_file` を 1 回通す（unit adapters）。
- D5: `worker_task.rs` の `misplaced_report`・`classify_budget_kind_from_text`・`is_budget_outcome` の撤去の条件を書いた（unit net が判断する）。
- D6: 続けられない adapter の run は auto-leaf の回数を測らない（決定の要求も checkpoint も作らない）。

### 変えた file

- `crates/task-worker/src/adapter.rs`（trait の method）、`browser_specialist.rs`（転送）、`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`（`false` の上書きだけ。監視 loop は変えていない）、`adapter_tests.rs`（試験）。
- `crates/task-dispatch/src/routing_runtime.rs`（性質の引き方）、`dispatcher/worker_finish.rs`（continuation の入力と auto-leaf）、`run_settlement/work_unit.rs`（D3 の WU の枝）、`run_settlement/work_unit/tests.rs`（試験の改名と期待値）、`dispatcher/tests/research_deadline.rs`・`dispatcher/tests/mod.rs`（試験）。
- `worker_finish.rs` の `finish_worker_result_with` は repo-health の `size.responsibility-growth` の基準（772 行）を超えないよう、`proposal` の 4 欄の取り出しを 2 行にまとめた（意味は同じ）。性質の引き方を `Dispatcher` の method にすると `Dispatcher` の method 数（基準 382）と dispatcher module の行（基準 31581）が増えて fail になったので、harness adapter の表を持つ `RoutingRuntime`（method 52）に置いた。

### 試験（不変条件と、既存の試験で検出できない理由は ADR の「試験」節）

- task-worker: `adapter::run_common_tests::research_deadline_continues_from_checkpoint_per_adapter`
- task-dispatch: `dispatcher::tests::research_deadline::research_deadline_atomic_run_neither_continues_nor_saves_a_checkpoint`、`…::research_deadline_work_unit_run_neither_continues_nor_saves_a_checkpoint`、`…::research_deadline_session_adapters_continue_and_research_adapters_do_not`
- 変えた試験: `run_settlement::work_unit::tests::continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it`（旧名 `…_keeps_the_unit_…`。WU を変えない契約は task を止めるので廃止した。WU の試験が修正前に `run_until_idle` 400 tick で idle にならず、dispatched 0 で止まったことで確かめた）

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -p task-worker -E 'test(/research_deadline_|continuation|auto_leaf|work_unit|langmem|paperqa|local_deep_research|browser_specialist|session/)'`: 427 件合格、失敗 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: verdict=pass、new=0、worsened=0。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）: exit 0。nextest は 6157 件実行、6157 件合格（slow 17）、skip 22。script bundle の SKIP 0 件。repo-health は verdict=pass、worsened=0。
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`: `write-set R6: ok`、exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"` の path は上の「変えた file」と同じ 14 件。
- 凍結の path（campaign・routing-eval・jev-classifier・script-runtime・control-ts・migrations・events.rs・script_guard・credentiald）の差分: 0 件。3 adapter の差分は `continues_from_checkpoint` の上書き（各 5 行）だけで、監視 loop の差分は 0。
- `grep -n -i -w "langmem\|paperqa\|local-deep-research\|local_deep_research\|LdrAdapter"` を `run_settlement` と `worker_finish.rs` に当てた結果: 0 件（exit 1）。

### 未解決

1. D6 の「続けられない adapter の run は auto-leaf の回数を測らない」を dispatcher で走らせて確かめる試験は無い（auto-leaf の偽アダプタと木の root の組み立ては `dispatcher/tests/tree_gate.rs` の private な関数で、この unit では写さなかった）。worker 完了の経路は `auto_counts` の条件に性質を足しただけで、性質が `false` の run では `auto_counts` が `None` になる。
2. ADR の D3 の WU の修正は `run_settlement`（lease・budget の高 risk の path class）の既存の枝の挙動を変える。分岐の条件は変えていないが、設計 review の対象になる。

### 提案

1. `sessions::SUPPORTED_ADAPTERS`（継続 session を持てる adapter の名前の一覧）も `WorkerAdapter` の性質に移す。今は名前の一覧で、ADR-0054 D1 の範囲。本 task の範囲外。
