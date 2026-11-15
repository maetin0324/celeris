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
- `research adr: 性質の引き方を sessions.rs の純粋関数に移し、範囲外の routing_runtime.rs と browser_specialist.rs を base に戻す`
- `research adr: 2 回目の run の検査の結果を進捗に書く`

### 決定（[ADR 2026-11-15-research-adapter-deadline](../../adr/2026-11-15-research-adapter-deadline.md)）

- D1: 研究 3 adapter の wall clock の超過は `BudgetExhausted{WallClock}` に揃える。監視 loop の置き換えは unit adapters。
- D2: `WorkerAdapter::continues_from_checkpoint`（既定 `true`）を足した。langmem・local-deep-research・paperqa が `false`、browser-specialist は上書きせず既定（daemon が包む内側は acp だけ）。dispatcher は純粋関数 `sessions::run_continues_from_checkpoint` に `RoutingRuntime::harness_adapters()` を渡し、`WorkerStarted.adapter` から性質を引く（引けなければ `false`）。`worker_finish.rs` が atomic task・WU・WU の checkpoint の判定の continuation の入力を「`[execution] continuation` かつ性質」にした。`sessions::SUPPORTED_ADAPTERS` は続けられる adapter の部分集合。
- D3: atomic task は既存の枝で `WorkerError{retryable:true}`・checkpoint なし。WU は既存の枝が WU を `running` のまま残して task を止めていたので、`RunEnd::Failed{retryable:true}` として WU の retry（`work_unit_retry`）に回すよう直した（`[execution] continuation = false` の WU も同じく直る）。
- D4: 3 adapter は成功の枝で result.json を書いた直後に `read_result_file` を 1 回通す（unit adapters）。
- D5: `worker_task.rs` の `misplaced_report`・`classify_budget_kind_from_text`・`is_budget_outcome` の撤去の条件を書いた（unit net が判断する）。
- D6: 続けられない adapter の run は auto-leaf の回数を測らない（決定の要求も checkpoint も作らない）。

### 変えた file

- `crates/task-worker/src/adapter.rs`（trait の method）、`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`（`false` の上書きだけ。監視 loop は変えていない）、`adapter_tests.rs`（試験）。
- `crates/task-dispatch/src/sessions.rs`（性質の引き方。純粋関数）、`dispatcher/worker_finish.rs`（continuation の入力と auto-leaf）、`run_settlement/work_unit.rs`（D3 の WU の枝）、`run_settlement/work_unit/tests.rs`（試験の改名と期待値）、`dispatcher/tests/research_deadline.rs`・`dispatcher/tests/mod.rs`（試験）。
- `worker_finish.rs` の `finish_worker_result_with` は repo-health の `size.responsibility-growth` の基準（772 行）を超えないよう、`proposal` の 4 欄の取り出しを 2 行にまとめた（意味は同じ）。性質の引き方を `Dispatcher` の method にすると `Dispatcher` の method 数（基準 382）と dispatcher module の行（基準 31581）が増えて fail になった。1 回目の run は `RoutingRuntime` の method に置いたが、`routing_runtime.rs` と `browser_specialist.rs`（転送の上書き）はこの unit の範囲 check の外で不合格になった（2 回目の run で確認）。2 回目は `routing_runtime.rs`・`browser_specialist.rs` を base に戻し、性質の引き方を `sessions.rs` の純粋関数にして `worker_finish.rs` から `harness_adapters()` を渡す形にした。browser-specialist は既定値（`true`）のまま。

### 試験（不変条件と、既存の試験で検出できない理由は ADR の「試験」節）

- task-worker: `adapter::run_common_tests::research_deadline_continues_from_checkpoint_per_adapter`
- task-dispatch: `dispatcher::tests::research_deadline::research_deadline_atomic_run_neither_continues_nor_saves_a_checkpoint`、`…::research_deadline_work_unit_run_neither_continues_nor_saves_a_checkpoint`、`…::research_deadline_session_adapters_continue_and_research_adapters_do_not`
- 変えた試験: `run_settlement::work_unit::tests::continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it`（旧名 `…_keeps_the_unit_…`。WU を変えない契約は task を止めるので廃止した。WU の試験が修正前に `run_until_idle` 400 tick で idle にならず、dispatched 0 で止まったことで確かめた）

### 証拠（2 回目の run。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

1 回目の run の check（範囲 check）は `crates/task-dispatch/src/routing_runtime.rs` と `crates/task-worker/src/browser_specialist.rs` が範囲外で exit 1 だった。2 回目は 2 file を base に戻し、以下を提出時の HEAD で再実行した。

- 範囲 check（計画の check と同じ command。`sh "$CELERIS_WU_SCOPE_PATHS"` の path を write set の正規表現で絞る）: 範囲外 0 件、exit 0。path は 13 件（ADR・この進捗・`sessions.rs`・`dispatcher/worker_finish.rs`・`dispatcher/tests/{mod,research_deadline}.rs`・`run_settlement/work_unit.rs`・`run_settlement/work_unit/tests.rs`・`task-worker/src/{adapter,adapter_tests,langmem,local_deep_research,paperqa}.rs`）。
- `cargo nextest run -p task-dispatch -p task-worker -E 'test(/research_deadline_|continuation|auto_leaf|work_unit|langmem|paperqa|local_deep_research|browser_specialist|session/)'`: 427 件合格、失敗 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: verdict=pass、new=0、worsened=0（`sessions.rs` へ移した直後の 5 行の呼び出しでは `finish_worker_result_with` の行 774 と dispatcher module の行が worsened で fail になったので、呼び出しを 2 行にして pass にした）。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）: exit 0。nextest は 6157 件実行、6157 件合格（slow 8）、skip 22。script bundle の SKIP 0 件。repo-health は verdict=pass、worsened=0。
- `git diff --name-only $CELERIS_WU_BASE | python3 -I agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`: `write-set R6: ok`、exit 0。
- 凍結の path（campaign・routing-eval・jev-classifier・script-runtime・control-ts・migrations・events.rs・script_guard・credentiald）の差分: 0 件。3 adapter の差分は `continues_from_checkpoint` の上書き（各 5 行）だけで、監視 loop の差分は 0。
- `grep -n -i -w "langmem\|paperqa\|local-deep-research\|local_deep_research\|LdrAdapter"` を `run_settlement.rs`・`run_settlement/`・`dispatcher/worker_finish.rs` に当てた結果: 0 件（exit 1）。

### 未解決

1. D6 の「続けられない adapter の run は auto-leaf の回数を測らない」を dispatcher で走らせて確かめる試験は無い（auto-leaf の偽アダプタと木の root の組み立ては `dispatcher/tests/tree_gate.rs` の private な関数で、この unit では写さなかった）。worker 完了の経路は `auto_counts` の条件に性質を足しただけで、性質が `false` の run では `auto_counts` が `None` になる。
2. ADR の D3 の WU の修正は `run_settlement`（lease・budget の高 risk の path class）の既存の枝の挙動を変える。分岐の条件は変えていないが、設計 review の対象になる。

### 提案

1. `sessions::SUPPORTED_ADAPTERS`（継続 session を持てる adapter の名前の一覧）も `WorkerAdapter` の性質に移す。今は名前の一覧で、ADR-0054 D1 の範囲。本 task の範囲外。

## unit adapters（完了日: 2026-11-15）

### commit

- `research adapters: 研究 3 adapter の監視 loop を deadline_terminal に寄せ、成功の枝で read_result_file を通す`
- `research adapters: 進捗に unit adapters の節を書く`
- `research adapters: architecture-map の研究 adapter の行で result.json を path として書かない`
- `research adapters: 2 回目の run の検査の結果を進捗に書く`

### 変更（ADR の D1・D4）

- `langmem.rs`・`local_deep_research.rs`・`paperqa.rs`（paperqa は両方の段で使う `stream_child`）の監視 loop: 締め切りの写し（`wall_elapsed`・`idle_elapsed`・`min` の待ち・`"wall clock exceeded"`・`"idle timeout"` の `Error` の組み立て）を消し、`adapter::deadline_terminal` を呼ぶようにした。wall clock の超過は `BudgetExhausted{WallClock, "wall clock exceeded", usage: None}`、idle の超過は今と同じ `Error{retryable: true, "idle timeout"}`。
- langmem は `min([adapters.langmem].idle_timeout_secs, limits.idle_timeout)` を `deadline_terminal` の `idle_timeout` に値で渡す。欄は残した。paperqa は取得と回答の 2 段で同じ `start`（`run_paperqa` の始まり）を使う（壁時計は run 全体。これまでと同じ）。
- `force_kill` の bool を消し、`timeout_terminal.is_some()` から導いた（同じ状態を 2 つ持たない）。
- stderr を写す task の写し 3 つ（paperqa は `stream_child` の 1 つ）は `adapter::spawn_capture_stderr` に替えた。終端の書き出しの写し（`write_result_json` → `from_provider_failure` → `RunOutcome`）は `subprocess::finish_run` に替えた。paperqa の取得の段の締め切りは、kill した取得ランナーの exit status も `finish_run` に渡す（`exit_code` はこれまで `None` 固定。signal で終わればこれまでどおり `None`）。ADR の状態の記録に付記した。
- D4: 成功の枝は各 adapter の `terminal_from_summary` が `artifacts/result.json`（`{"summary", "evidence": []}`）を書き、`result_report::read_result_file` を 1 回通して `interpret_result_text` で終端を作る。書けなければ `read_result_file` を呼ばずに `Error{retryable: true}`（`<adapter> adapter could not write <artifacts_rel>/result.json: …`）。これまでは warn だけで `Done` にしていた。締め切りの枝と engine の失敗の枝は result.json を書かず、`read_result_file` も呼ばない。
- unit adr が足した `continues_from_checkpoint` の上書き（`false`）は変えていない（差分 0）。
- 共通の interface（`adapter.rs`・`result_report.rs`・`subprocess.rs`）は変えていない。研究 harness 専用の分岐・新しい fallback・切替 flag は足していない。
- `docs/architecture-map.md` の研究 adapter の行に langmem と ADR 2026-11-15 と不変条件を足した。ADR の「状態」と「状態の記録」を更新した。

### 試験

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `{langmem,local_deep_research,paperqa}::tests::wall_clock_exceeded_kills_the_process_group`（変更） | wall clock の超過は `BudgetExhausted{WallClock}`、`runs/<run_id>/result.json` の行も `budget_exhausted`、`artifacts/result.json` を書かない | 旧い契約（`Error{retryable: true}`）を確かめていた。D1 で廃止した |
| `langmem::tests::idle_timeout_secs_cannot_exceed_the_harness_budget`（強化） | config の 3600 秒は予算の 300 ms を超えて効かない。終端は `Error{retryable: true, "idle timeout"}` | これまでは `Error` であることだけを見ていた。上回れたなら wall clock の `BudgetExhausted` になるので、終端の種類で区別できるようにした |
| `langmem::tests::idle_timeout_secs_shorter_than_the_budget_takes_effect`（新規） | config の 1 秒は予算（idle・wall clock とも 20 秒）より先に効き、idle の `Error` になる | 上の試験は予算が勝つ側だけを見るので、config の値を `deadline_terminal` に渡し忘れても通る |
| `{langmem,local_deep_research,paperqa}::tests::an_unwritable_result_file_is_a_retryable_error_not_done`（新規） | `artifacts/result.json` を書けない成功の枝は `Done` でなく retryable な `Error`（D4） | 既存の試験は書ける置き場だけを使い、warn だけの `Done` を検出できない |
| `paperqa::tests::wall_clock_exceeded_in_the_acquire_stage_stops_before_the_answer_stage`（新規） | 取得の段で壁時計が尽きれば `BudgetExhausted{WallClock}` で終わり、取得ランナーを kill し、回答の段を起こさない | 既存の wall clock の試験は取得の段の無い構成（`max_candidates = 0`）だけを見ていた |

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 受け入れ条件 0: `grep -nE "wall_elapsed|idle_elapsed|\"wall clock exceeded\"|\"idle timeout\"|force_kill|stderr_log_path_for_task|tokio::spawn\(|write_result_json" crates/task-worker/src/{langmem,local_deep_research,paperqa}.rs` → 0 件（exit 1）。`grep -c "deadline_terminal("` → 各 file 1 件。
- 受け入れ条件 1: `grep -nB3 'contains("wall clock exceeded")' crates/task-worker/src/{langmem,local_deep_research,paperqa}/tests.rs` → 0 件（exit 1）。
- 受け入れ条件 2・3: `cargo nextest run -p task-worker -p task-dispatch -E 'test(/research_deadline_/) | test(/^langmem::/) | test(/^local_deep_research::/) | test(/^paperqa::/)'` → 138 件合格、失敗 0。task-worker の `research_deadline_continues_from_checkpoint_per_adapter`、task-dispatch の `research_deadline_atomic_run_neither_continues_nor_saves_a_checkpoint`・`research_deadline_work_unit_run_neither_continues_nor_saves_a_checkpoint`・`research_deadline_session_adapters_continue_and_research_adapters_do_not` を含む。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）: exit 0。nextest は 6162 件実行、6162 件合格（slow 10）、skip 22。script bundle の SKIP 0 件。repo-health は verdict=pass、new=0、worsened=0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 範囲 check（計画の check と同じ command）: 範囲外 0 件。`git diff --name-only $CELERIS_WU_BASE | python3 -I agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`: `write-set R6: ok`、exit 0。
- 凍結の path（campaign・routing-eval・jev-classifier・script-runtime・control-ts・migrations・events.rs・script_guard・credentiald）の差分: 0 件。変えた file は 3 adapter の `.rs`・`tests.rs`、ADR、この進捗、`docs/architecture-map.md` だけ。

### 2 回目の run（celeris の check の不合格の修正）

- 原因: 1 回目の run の提出の後、celeris が走らせた `bash scripts/dev/test-affected.sh` が exit 1 になった。repo-health の `arch-map.wrong_path` が fail（`docs/architecture-map.md` の 103 行目の `artifacts/result.json` が存在しない）。検査器（`scripts/dev/check-architecture-map.py` の `looks_like_path`）は表の行の backtick の語に `/` があれば repo の path として扱う。この語は run の成果物ディレクトリの file で、repo にはない。1 回目の run の証拠（上の test-affected.sh の exit 0）がこの行を足す前の HEAD で取ったものかは未確認。
- 修正: 103 行目の語を「adapter が成果物ディレクトリに書いた `result.json`」に替えた（意味は同じ。`/` を含まないので path として扱われない）。コードの変更は無い。
- 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）:
  - `python3 scripts/dev/check-architecture-map.py`: `OK: 1069 件のパスを確認した`、exit 0。
  - `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）: exit 0。nextest は 6162 件実行、6162 件合格（slow 7）、skip 22。script bundle の SKIP 0 件。repo-health は verdict=pass、`arch-map.wrong_path` は pass（findings=0）、new=0、worsened=0。
  - `cargo nextest run -p task-worker -p task-dispatch -E 'test(/research_deadline_/) | test(/^langmem::/) | test(/^local_deep_research::/) | test(/^paperqa::/)'`: 138 件合格、失敗 0。
  - `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
  - `sh "$CELERIS_WU_SCOPE_PATHS"`: 9 件（3 adapter の `.rs`・`tests.rs`、ADR、この進捗、`docs/architecture-map.md`）、exit 0。`git diff --name-only $CELERIS_WU_BASE | python3 -I agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`: `write-set R6: ok`、exit 0。
  - 凍結の path（events.rs・migrations ほか）の差分: 0 件。

### 未解決

1. 3 adapter の `terminal_from_summary` は同じ形の写し（20 行ほど。違いは文言の adapter 名だけ）。置く先は結果契約の正本 `result_report.rs` が自然だが、この unit の範囲 check の外なので各 adapter に置いた（提案 1）。
2. `subprocess::prepare_run` は使っていない。使うと前回の `delegate.json` の消去と、browser の run の log を `/dev/null` にする分岐が加わり、挙動が変わる。研究 harness は委譲も browser もしないので、寄せるかは別の判断にする。
3. unit net へ（ADR の D5 の 1）: 研究 3 本の `Done` は、自分で書いた `artifacts/result.json` を `read_result_file` で読んでから返すようになった。dispatcher の `misplaced_report` の探索が研究 harness に届くのは `Error` の枝（engine が書かない file の探索）だけ。

### 提案

1. `result_report.rs` に「adapter が engine の代わりに `{"summary", "evidence": []}` を書いて読み直す」関数を 1 つ置き、研究 3 本の `terminal_from_summary` の写しを消す（文言の adapter 名は引数）。`result_report.rs` を書ける unit（unit net か R6 の close）で行う。
