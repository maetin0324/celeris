---
title: 健全性v2 R1 result-contract — result.json の解釈を結果契約 1 つにする
tasks: [01M6TYVJSZDSCB697193J01FYW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R1 result-contract — result.json の解釈を結果契約 1 つにする

完了日: 2026-11-10

WorkUnit `result-contract`（[ADR 2026-11-10 harness adapter の結果契約と実行共通処理](../../adr/2026-11-10-adapter-result-contract.md) の D1・D3・(4) 4a/4c）。基準は `$CELERIS_WU_BASE`（slices-check は `git merge-base HEAD main`）。

## 変更
- 件名「R1 result-contract: result.json の解釈を result_report.rs の結果契約 1 つにし、5 adapter の写しを消す」の commit
  - `crates/task-worker/src/result_report.rs` に結果契約の正本を置いた: `pub(crate) struct ResultFile`、`pub(crate) fn lenient_evidence`、`pub(crate) fn interpret_result_text(text, artifacts_rel, usage) -> Terminal`（本文を `Value` に 1 回だけ parse。優先順位は `question`（文字列）> `wait`（`adapter::wait_terminal`）> `summary` > `yield`。JSON でない・欄の型が違う・どの欄も無い・`wait` が不正は `Error{retryable: true, usage: None}`。文言は現行のまま）、`pub(crate) enum ResultFileRead { Misplaced(Box<Terminal>), Missing, Text(String) }`、`pub(crate) async fn read_result_file(req, run_started)`（`requires_result_file()` の run は先に `result_location::misplaced_report`）。
  - claude_code・codex・pi・acp・aider から `struct ResultFile`・`fn lenient_evidence`・優先順位の if 連鎖・「has neither」の `Error` を消し、`read_result_file` と `interpret_result_text` を呼ぶようにした。adapter に残したのは provider 固有の部分だけ（ADR (3)）: claude_code の `ResultMeta` の判定（`error_max_turns`・context 超過・供給側失敗の分類）と `RESULT_JSON_MISSING_MARKER`、codex の「codex exited without」と Phase 112 D3 の書き戻し、pi・aider の「exited without」、acp の `max_turn_requests` と「acp agent stopped (…) without」。
  - `adapter::result_file_wait` を消した。`adapter::wait_terminal` は `pub fn wait_terminal(value: &serde_json::Value, usage: Option<Usage>) -> Option<Terminal>` のまま残した（`task-dispatch` の `dispatcher.rs`・`subprocess.rs` が使う）。
  - 試験: 共通契約の試験を `crates/task-worker/src/result_report_tests.rs` に足し、adapter の契約の写しを消した（下の「試験」）。
- 件名「R1 result-contract: pi・acp・aider の誤配置の窓を監視 loop の開始から取り、run 関数の行を増やさない」の commit
  - pi・aider・acp は `read_result_file` に渡す時刻を、監視 loop の `start: Instant` を壁時計に直した値（`SystemTime::now() - start.elapsed()`）にした。`run_pi`・`run_acp`・`run_aider` は基準の時点で 200 行を超えており、`let run_started = …` の 1 行でも `size.responsibility-growth` が fail になったため（下の「検査」）。

## 観測できる挙動の変化
ADR が統一を決めたもの（D1「誤配置の回収は 1 つの関数」、(1d) 8「二重の parse」）だけ。
1. pi・acp・aider も adapter の時点で誤配置を回収する（D1）。候補が複数なら `Error{retryable: false}`（`MISPLACED_RESULT_MARKER`）。候補が 1 つなら置き場に写して読む。後者は task の終端も変わる: これまで adapter は「pi exited without …」の `Error{retryable: true}` を返し、dispatcher（`worker_task.rs`）の `misplaced_report` は写すだけで `None` を返すので `Error` のまま再試行していた。今は claude_code・codex と同じく写した本文で終端を作る（例: `Done`）。
2. pi・acp・aider の誤配置の候補の窓は、run の開始ではなく監視 loop の開始（spawn の後）から 2 秒前まで。worker が result.json を書くのは spawn の後なので、候補から落ちる file は無い。claude_code・codex は従来どおり run の開始。
3. 本文の parse を 1 回にした（(1d) 8）ので、serde の誤りの細部が 2 点変わる。(a) 欄の型が違うとき（例 `{"question": 1}`）の文言の末尾に serde の「at line L column C」が付かなくなった（接頭辞「`<rel>/result.json is not valid JSON: `」と `Error{retryable: true}` は同じ）。(b) 同じ鍵が 2 回ある本文（例 `{"summary":"a","summary":"b"}`）は「duplicate field」の `Error` ではなく、後の値で読む。ADR は「変化なし」と書いているので、record で付記する（提案 1）。
4. codex の Phase 112 D3 の回収は、書き戻した本文を `interpret_result_text` で直接読む（従来は file を読み直していた。本文は同じなので終端は同じ）。

`Terminal` の種類・`retryable`・usage の渡し方（Done・Question・Yielded・Waiting に載せ、Error には載せない。acp は `None`）は変えていない。

## 試験
足した（`result_report::result_contract_tests`。守る不変条件と既存で検出できない理由は各試験の doc comment と ADR (4) 4a）:
- `result_contract_priority_order`・`result_contract_lenient_evidence`・`result_contract_invalid_json_is_retryable_error`・`result_contract_no_known_field_is_retryable_error`・`result_contract_usage_passthrough`
- `read_result_file_recovers_strays_only_for_runs_that_require_the_file`（誤配置の回収を要る run だけが通すこと、候補 1 つは写して読む・複数は `Misplaced`・無ければ `Missing`）

消した（契約の写し。ADR (4) 4c の理由: 正本が 1 つになり、下の共通試験が同じ不具合を検出する）:

| adapter | 消した試験 | 代わりの共通試験 |
|---|---|---|
| claude_code | `invalid_result_file_json_is_retryable_error` | `result_contract_invalid_json_is_retryable_error` |
| claude_code | `malformed_evidence_in_result_file_does_not_fail_the_run` | `result_contract_lenient_evidence`（prompt の「plain strings are not accepted」の確認は `build_prompt_includes_objective_criteria_and_result_file_instructions` に移した） |
| claude_code | `question_in_result_file_blocks_task` | `result_contract_priority_order`・`result_contract_usage_passthrough` |
| claude_code | `result_yield_becomes_terminal_yielded` | `result_contract_priority_order` |
| claude_code | `result_wait_becomes_terminal_waiting`（unit wait-line が前半を戻した） | run dir の wait の行: `run_common_finish_run_writes_wait_line_for_waiting`（`finish_run` の写像と `wait_terminal` での読み直し）と、戻した `result_wait_becomes_terminal_waiting`（結線: `Terminal::Waiting` と `runs/run-wait/result.json` の行と jobs）。不正な wait と question の優先: `result_contract_priority_order`・`result_contract_no_known_field_is_retryable_error`。当初の代わりとした dispatcher の `cluster_job_wait` の試験は run dir の行を通らないので代わりにならなかった（[wait-line.md](wait-line.md)） |
| codex | `invalid_result_file_json_is_retryable_error` | `result_contract_invalid_json_is_retryable_error` |
| codex | `malformed_evidence_in_result_file_does_not_fail_the_run` | `result_contract_lenient_evidence` |
| codex | `question_in_result_file_blocks_task` | `result_contract_priority_order` |
| codex | `result_yield_becomes_terminal_yielded` | `result_contract_priority_order` |
| acp | `question_in_result_file_blocks_task` | `result_contract_priority_order` |
| acp | `result_yield_becomes_terminal_yielded` | `result_contract_priority_order` |
| aider | `result_yield_becomes_terminal_yielded` | `result_contract_priority_order` |
| pi | `pi_adapter_result_question_yield_and_invalid_json` の yield・invalid の 2 組 | `result_contract_priority_order`・`result_contract_invalid_json_is_retryable_error`（残りの question の 1 組は `pi_adapter_result_file_question_becomes_terminal_question` に改名して結線の試験にした） |

残した結線の試験（fake の harness の実行で結果契約を通る）: claude_code・codex・acp の `happy_path_progress_and_done_from_result_file`、aider の `a_question_in_result_json_becomes_terminal_question`、pi の `pi_adapter_result_file_question_becomes_terminal_question`。provider 固有の分類の試験（result.json が無いとき・stale・cos_chat・Phase 112・background task）は全部残した。

## 検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
clippy・fmt・test-affected は件名「R1 result-contract: 進捗に結果契約の実装・観測できる変化・試験の入れ替え・検査を書く」の commit（clean な tree）で実行した。その後の commit はこの file の 1 行の訂正だけ。
- `cargo nextest run -p task-worker -E 'test(result_contract_)'` → exit 0、6 passed（共通契約の 5 件と、同じ module `result_contract_tests` にある `read_result_file_recovers_strays_only_for_runs_that_require_the_file`）。
- `cargo nextest run -p task-worker` → exit 0、1265 passed。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。nextest 5847 passed（22 skipped）。repo-health は `verdict=pass`、`fail_grade=0`、`new=0`、`worsened=0`。
  - 1 回目（commit 1 の後）は `size.responsibility-growth` が 3 件 fail した（`pi::run_pi` 326 行・`acp::run_acp` 730 行・`aider::run_aider` 206 行。閾値 200 を超えた関数の行が基準より増えた）。commit 2 で `run_started` の行を消して直した。
- `cargo clippy --workspace -- -D warnings` → exit 0（途中で `large_enum_variant` を指摘されたので `ResultFileRead::Misplaced` を `Box<Terminal>` にした）。
- `cargo fmt --all -- --check` → exit 0。
- `grep -rlE 'struct ResultFile\b|fn lenient_evidence\b' crates/task-worker/src` → `crates/task-worker/src/result_report.rs` の 1 file だけ（受け入れ条件 0）。
- `grep -rlF "nor 'yield'" crates --include=*.rs` → `result_report.rs` と `result_report_tests.rs` だけ（受け入れ条件 1）。
- `grep -rn "fn result_file_wait" crates/` → 0 件。`grep -n "pub fn wait_terminal" crates/task-worker/src/adapter.rs` → 1 件。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-worker/src/{result_report,result_report_tests,adapter,claude_code,codex,pi,acp,aider}.rs`、5 adapter の `tests.rs`、この file だけ。`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`・`crates/task-dispatch/` は変えていない。

## 未解決
- 目的文は優先順位を「wait > question > summary > yield」と書くが、ADR と現行の code は `question`（文字列）> `wait`。ADR の順（現行どおり）で実装した（task の進捗の「未解決」と同じ）。

## 提案
1. record で ADR に付記する: (1d) 8「二重の parse → 変化なし」は、serde の誤りの文言の位置の末尾と重複鍵の扱いの 2 点で変わった（上の「観測できる挙動の変化」3）。pi・acp・aider の誤配置の窓は監視 loop の開始から取る（同 2）。
2. run-common へ: `run_pi`・`run_acp`・`run_aider`（と claude_code・codex の run 関数）は基準の時点で閾値 200 行を超えている。repo-health の `size.responsibility-growth` は、これらの関数の行が基準より 1 行でも増えると fail にする。`prepare_run` などへ寄せるときは関数の行が減る向きにする。
