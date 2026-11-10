---
title: 健全性v2 R1 run-common — 実行共通処理を 1 か所にし、5 adapter の写しを消す
tasks: [01M6TYVJSZDSCB697193J01FYW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R1 run-common — 実行共通処理を 1 か所にし、5 adapter の写しを消す

完了日: 2026-11-10

WorkUnit `run-common`（[ADR 2026-11-10 harness adapter の結果契約と実行共通処理](../../adr/2026-11-10-adapter-result-contract.md) の D2・D3・(4) 4b）。result-contract の上に作った。基準は `$CELERIS_WU_BASE`（slices-check と repo-health は `git merge-base HEAD main`）。

## 変更
- 件名「R1 run-common: run の準備・締め切り・stderr・終端・cost 推定を adapter.rs に寄せ、5 adapter の写しと preamble::worktree_note を消す」の commit と、その後の 3 件（「deadline_terminal の終端を Box にする」「prepare_run・finish_run を subprocess.rs に置き、adapter.rs からの依存の循環を作らない」「aider の module doc の cost 推定を adapter::with_estimated_cost に向ける」）。
- `crates/task-worker/src/adapter.rs` に置いたもの:
  - `deadline_terminal(start, last_activity, wall_clock, idle_timeout) -> Result<Duration, Box<Terminal>>`: wall clock の超過は `BudgetExhausted{WallClock, "wall clock exceeded", usage: None}`、idle の超過は `Error{retryable: true, "idle timeout", usage: None}`、両方なら wall clock、どちらも超えていなければ残りの小さい方。
  - `spawn_capture_stderr(child, path, run_id) -> Result<JoinHandle<()>, AdapterError>`: stderr を file に写す task。
  - `with_estimated_cost(usage, model)`: `cost_usd` が `None` で model が分かるときだけ `task_core::estimate_cost_usd` で埋める。pi は呼ばない（ADR D2 の表 2）。
- `crates/task-worker/src/subprocess.rs` に置いたもの（ADR は `adapter.rs` と書いた。下の「ADR との違い」）:
  - `RunPaths { run_dir, stdout_log, stderr_log, result_path, artifacts_rel }` と `prepare_run(req, run_id, stdout_name)`: run dir の作成、前回の `artifacts/result.json` の削除、`clear_delegate_file`、`campaign_pin::enforce`、`write_run_request`。browser の run は stdout・stderr の log を `/dev/null` にする。
  - `finish_run(run_dir, terminal, provider_failure, exit_status)`: `write_result_json` → `Error` と供給側失敗の組は `Err(AdapterError::from_provider_failure)` → `Ok(RunOutcome)`。
- 5 adapter（claude_code・codex・pi・acp・aider）から消した写し: run dir の準備（作成・log の path・result.json の削除・delegate の削除・enforce・request.json）、締め切りの計算（wall clock と idle の if 2 つと待ち時間の式）、stderr を写す `tokio::spawn`、終端の書き出し（`write_result_json`・供給側失敗の `Err`・`RunOutcome`）、cost の関数 3 つ（claude_code の `usage_with_cost`、codex の `with_estimated_cost`、aider の `usage_with_cost`）。acp の private な `finish_run` は `finish_session` に改名し、最後に `subprocess::finish_run` を呼ぶ。
- adapter に残したもの（ADR (3)）: provider 固有の wire・認証・session の再開、stream の解析、result.json が無いときの分類、acp の `session/cancel`、codex の resume 失敗の fresh やり直し（`stdout.resume_retry.jsonl` と、やり直し前の result.json・delegate の削除）、claude_code の `RESULT_JSON_MISSING_MARKER` の `Err(AdapterError::Other)`（`finish_run` の後）と browser の文言の伏せ字（`finish_run` の前）、pi の wall clock の終端に `stream.usage` を載せる処理、claude_code の `exploration` の累積 usage。
- `crates/task-worker/src/preamble.rs` の `pub fn worktree_note` を消した（dup-candidates `unref-worktree-note`）。
- `adapter::wait_terminal` の公開の署名と `subprocess.rs` の既存 helper の署名は変えていない。`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`・`crates/task-dispatch/` は変えていない。

## ADR との違い
1. `RunPaths`・`prepare_run`・`finish_run` は `adapter.rs` でなく `subprocess.rs` に置いた。`adapter.rs` に置くと `adapter` が `campaign_pin`・`delegate_file`・`subprocess` を呼び、3 つとも `adapter` の型（`AdapterError`・`EventSink`・`Terminal`）に依存するので、repo-health の `deps.cycle` が base の 3 単位（adapter・tool_policy・progress）から 6 単位に悪化して fail した（1 回目の `check-health.sh` が exit 1）。`subprocess` は既に `adapter` と `campaign_pin` に依存しており、`subprocess` に依存し返す module は無いので、ここに置けば循環は増えない。`deadline_terminal`・`spawn_capture_stderr`・`with_estimated_cost` は新しい依存を作らないので `adapter.rs` に置いた。
2. `deadline_terminal` の終端は `Box<Terminal>`（clippy `result_large_err`）。acp の `WaitOutcome::TimedOut` は元から箱に入れていたのでそのまま渡す。
3. `spawn_capture_stderr` は `Result<JoinHandle<()>, AdapterError>` を返す（stderr が pipe でないときの `AdapterError::Other("worker stderr was not piped")` を保つため）。
4. `prepare_run` は `stdout_name` を受ける（ADR D2 の表 4「現行の名前を adapter の指定で作る」）。

## 観測できる挙動の変化
ADR D2 の表で統一と決めたものだけ。
1. cost の推定は「欠けたときだけ埋める」になった（表 2）。claude_code・codex の stream の解析は `cost_usd: None` を置くので結果は同じ。
2. codex・pi・aider も browser の run の log を `/dev/null` にする（表 5）。これらには browser の run が届かないので、今の経路では変化は無い。
3. run の準備の順が変わった: `campaign_pin::enforce` と `request.json` の書き出しが、prompt を組む前（codex・pi・acp では必須規則 `required_rules::preamble_for_run` の読み取りの前、claude_code では画像の入力の組み立ての前、pi では skill の配送の前）に動く。必須規則が読めずに run が落ちたときも `request.json` は残る。両方が失敗する run では、返る誤りが campaign の pin の方になる。
4. stderr の写しの失敗の warn に `run <run_id>:` が付いた。

## 試験
足した（守る不変条件と既存の試験で検出できない理由は各試験の doc comment）:

| 試験 | 置き場 | 守る不変条件 | 既存で検出できなかった理由 |
|---|---|---|---|
| `run_common_deadline_wall_clock_is_budget_exhausted` | `adapter_tests.rs`（`adapter::run_common_tests`） | wall clock の超過は `BudgetExhausted{WallClock}`・文言 `wall clock exceeded`、idle も超過していれば wall clock が勝つ | adapter の試験は実時間の打ち切りを 1 経路ずつしか見ず、両方の超過の順を固定していなかった |
| `run_common_deadline_idle_is_retryable_error` | 同上 | idle の超過は retryable な `Error`・文言 `idle timeout`。超過が無ければ待ち時間は残りの小さい方 | 待ち時間の式は 5 か所の写しで、どれも試験されていなかった |
| `run_common_cost_estimate_keeps_reported_cost` | 同上 | 実測の cost は上書きしない。cost が無く model が単価表にあれば埋め、model が無い・単価表に無いなら `None` | claude_code・codex は常に上書きしていて、実測を優先する規則はどの adapter の試験にも無かった |
| `run_common_prepare_run_clears_previous_result_and_delegate` | `subprocess/tests.rs` | 前回の `result.json`・`delegate.json` を消し、`request.json` を書く。browser の run は log が `/dev/null` | 写しごとの処理で、codex・pi・aider には browser の分岐が無かった |
| `run_common_finish_run_maps_provider_failure_to_adapter_error` | 同上 | 終端を `runs/<run_id>/result.json` に書いてから、`Error` と供給側失敗の組だけを `Err` にする | adapter の試験は戻り値しか見ず、書き出しを飛ばしても検出できなかった |

消した試験は無い。adapter の結線の試験（result-contract の進捗の「残した結線の試験」と、provider 固有の分類・timeout の試験）は全部そのまま通る。

## 検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
clippy・fmt・test-affected・check-health は件名「R1 run-common: aider の module doc の cost 推定を adapter::with_estimated_cost に向ける」の commit（clean な tree）で実行した。その後の commit はこの file だけ。
- `cargo nextest run -p task-worker -E 'test(/run_common_|result_contract_/)'` → exit 0、11 passed（`run_common_` 5 件、`result_contract_` 5 件と同じ module の `read_result_file_recovers_strays_only_for_runs_that_require_the_file`）。
- `cargo nextest run -p task-worker` → exit 0、1270 passed（9 skipped）。途中の 1 回は `browser::launcher_run::tests::launcher_login::launcher_credential_*` の 4 件が落ちたが、adapter を通らない試験で、単独の再実行（`-E 'test(/launcher_credential_/)'`）で 37 件とも合格し、次の全体の実行でも合格した（並列の負荷による時間切れ）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。nextest 5852 passed（22 skipped）。repo-health は `verdict=pass`、`fail_grade=0`、`new=0`、`worsened=0`。
- `bash scripts/dev/check-health.sh` → exit 0。`size.responsibility-growth` は pass。200 行を超える run 関数の行（base → head）: `claude_code::run_claude_code` 561 → 487、`codex::run_codex` 337 → 306、`codex::run_codex_once` 346 → 314、`pi::run_pi` 321 → 276、`acp::run_acp` 729 → 698。`aider::run_aider` は 201 → 153 で閾値を下回った。1 回目（`prepare_run`・`finish_run` を `adapter.rs` に置いた版）は `deps.cycle` が worsened で exit 1（上の「ADR との違い」1）。
- `cargo clippy --workspace -- -D warnings` → exit 0（1 回目は `result_large_err` を指摘され、`Box<Terminal>` にした）。
- `cargo fmt --all -- --check` → exit 0。
- `grep -nE "estimate_cost_usd|fn (usage_with_cost|with_estimated_cost)|BudgetKind::WallClock|\"wall clock exceeded\"|\"idle timeout\"" crates/task-worker/src/{claude_code,codex,pi,acp,aider}.rs` → 0 件（受け入れ条件 0）。
- `grep -rn "fn worktree_note" crates/` → 0 件（受け入れ条件 1）。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（`write-set R1: ok`）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-worker/src/{acp,adapter,adapter_tests,aider,claude_code,codex,pi,preamble,subprocess}.rs`、`crates/task-worker/src/subprocess/tests.rs`、この file だけ。

## 未解決
- `cargo clippy -p task-worker --all-targets -- -D warnings` は `result_report_tests.rs:246`（result-contract の試験）の `field_reassign_with_default` で落ちる。repo の検査（`--all-targets` 無し）には出ない。この unit の範囲外として触っていない。
- `cargo clippy --workspace --all-targets` は `crates/task-core/src/chat/store_tests.rs` の `useless_conversion` でも落ちる（R1 の write_set の外。既存）。

## 提案
1. record で ADR に付記する: D2 の `RunPaths`・`prepare_run`・`finish_run` の置き場は `subprocess.rs`（`deps.cycle` を増やさないため）、`deadline_terminal` は `Box<Terminal>` を返す、`spawn_capture_stderr` は `Result` を返す、run の準備の順の変化（上の「観測できる挙動の変化」3）。
2. record で `result_report_tests.rs:246` を `RunContext { cos_chat: Some(Default::default()), ..Default::default() }` に直す（`--all-targets` の clippy を通すため）。
3. R6 へ: langmem・LDR・paperqa の run 関数も締め切りの写しを持つ。`deadline_terminal` は `idle_timeout` を値で受けるので、langmem の `[adapters.langmem].idle_timeout_secs` で縮めた値をそのまま渡せる。
