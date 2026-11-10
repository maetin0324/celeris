---
status: accepted
tasks: [01M6TYVJSZDSCB697193J01FYW]
---
# harness adapter の結果契約（ResultFile・lenient_evidence・result.json の解釈）と実行共通処理を 1 つにする

- 日付: 2026-11-10
- 状態: 実装済み（2026-11-10、健全性 v2 R1 の task 01M6TYVJSZDSCB697193J01FYW。実装は unit result-contract・run-common、記録は [R1 の進捗](../progress/2026-11-10-health-v2-r1.md)）。決定節と実装の食い違いは末尾の付記「実装との突き合わせ」に書く
- 関係: [ADR-0003 worker protocol](0003-worker-protocol.md)（`Terminal` と D4 の timeout の正規化）、ADR-0006 D3/D4（`artifacts/result.json` と `evidence` の寛容な読み方、`result` を観測できない run は file を信用しない）、ADR-0010 D5/D10（供給側失敗の分類、`runs/<run_id>/result.json`）、ADR-0036 D1/D2（`artifacts_dir`）、ADR-0061（静的単価表）、ADR-0072 D7/D9/E2（`BudgetExhausted`・`yield`・result.json 欠落の InfraRequeue）、ADR-0090 D1（`wait`）、[ADR 2026-11-06 誤配置した report の回収](2026-11-06-misplaced-report-recovery-generalized.md)、[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md)（R1 の scope・write_set）、[ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)

## 背景

harness adapter 5 本（`crates/task-worker/src/{claude_code,codex,pi,acp,aider}.rs`）は、worker が書く `artifacts/result.json` を読む `struct ResultFile`、`evidence` を寛容に読む `fn lenient_evidence`、終端を合成する `terminal_from_result`（acp は `terminal_from_result_file`）を、それぞれ自分の file に写して持つ。写しは 5 つとも同じ規約（ADR-0006 D3、ADR-0072 D9、ADR-0090 D1）を実装しているが、正本が無いので規約を変えるたびに 5 か所を直すことになり（ADR-0072 の `yield`、ADR-0090 の `wait` がそうだった）、試験も 5 か所に同じ内容が写されている。実行の共通処理（run dir の準備、stale な result.json の削除、wall clock と idle の締め切り、stderr の回収、usage の cost 推定、`runs/<run_id>/result.json` への書き出し）も同じ形で 5 回書かれている。

健全性 v2（ADR 2026-11-07-health-v2-baseline）の R1 は、この重複を「現行の契約」に沿って集約する工程である。本 ADR は (1) 現行挙動の棚卸し、(2) 共通の契約と置き場所、(3) provider 固有として残すもの、(4) 試験の配置、(5) R6 への引き継ぎを決める。新しい万能 adapter は作らない（親 task の原則）。

R1 の write_set（`slices.json` の R1）: `crates/task-worker/src/(adapter|result_report|result_location|protocol|route|preamble|claude_code|codex|pi|acp|aider|fake|subprocess).rs`、その下の試験、`crates/task-dispatch/src/dispatcher/worker_finish.rs`、ADR と進捗。`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`・`dispatcher/worker_task.rs` は外。

## (1) 現行挙動の表（2026-11-10 の main。R1 の base `$CELERIS_WU_BASE`）

### 1a. result.json の解釈

| 項目 | claude_code | codex | pi | acp | aider |
|---|---|---|---|---|---|
| 読む関数 | `terminal_from_result(artifacts_dir, rel, &ResultMeta, model)`。`result` message を観測した run だけが呼ぶ | `terminal_from_result(artifacts_dir, rel, usage, model)`。`turn.completed` を観測した run だけ | `terminal_from_result(artifacts_dir, rel, usage)`。exit 0 かつ `agent_end` を観測した run だけ | `terminal_from_result_file(artifacts_dir, rel, stop_reason)`。`session/prompt` の応答を得た run だけ | `terminal_from_result(artifacts_dir, rel, usage)`。timeout でなければ常に |
| 優先順位 | `question`（文字列）> `wait` > `summary` > `yield`。`wait` は `crate::adapter::result_file_wait` が先に見る | 同じ | 同じ | 同じ | 同じ |
| `wait` が不正 | `Error{retryable: true}`（`adapter::wait_terminal`、`usage: None`） | 同じ | 同じ | 同じ | 同じ |
| どの欄も無い | `Error{retryable: true}` 「`<rel>/result.json has neither 'summary', 'question' nor 'yield'`」、`usage: None` | 同じ | 同じ | 同じ | 同じ |
| JSON でない | `Error{retryable: true}` 「`<rel>/result.json is not valid JSON: <e>`」、`usage: None` | 同じ | 同じ | 同じ | 同じ |
| `evidence` | `lenient_evidence`: 配列の要素のうち `Evidence` に読めたものだけ。配列でなければ空 | 同じ | 同じ | 同じ | 同じ |
| `yield` | 生の JSON のまま `Terminal::Yielded{checkpoint}`。検証は dispatcher の `merge_checkpoint` | 同じ | 同じ | 同じ | 同じ |
| file の前の判定 | `is_error` または `subtype != success` なら file を読まない（`error_max_turns` → `BudgetExhausted{Turns}`、context 超過の語彙 → `BudgetExhausted{Context}`、他は `Error{retryable: true}` と供給側失敗の分類） | `turn.failed` は file を読まない（context 超過 → `BudgetExhausted{Context}`、他は `Error`） | `stream.error`・exit 非 0・`agent_end` 無しは file を読まない。Hashline extension の失敗は `Error{retryable: false}` | `stopReason == max_turn_requests` → `BudgetExhausted{Turns}`（file より優先）。Eof・RpcError は file を読まない | なし |
| result.json が無い | 「`claude exited without <rel>/result.json`」（`RESULT_JSON_MISSING_MARKER`）。`run` がこれを `Err(AdapterError::Other)` に倒し、dispatcher は InfraRequeue（attempts を消費しない。ADR-0072 E2）。cos_chat の成功 run は `result` の本文で Done。background task が残っていた成功 run は coding の execute なら `Yielded`（continuation）、他は `Error` に分類名を足す | 「`codex exited without …`」`Error{retryable: true}`。conversation・cos_chat の execute で exit 0 なら最後の agent message で Done。最後の message が `{"summary", "actions"}` の形なら result.json に書き戻してから読み直す（ADR-0054 Phase 112 D3） | 「`pi exited without …`」`Error{retryable: true}`。cos_chat なら最後の reply で Done | 「`acp agent stopped (<stopReason>) without …`」`Error{retryable: true}`。cos_chat の `end_turn` なら reply で Done | 「`aider exited without …`」`Error{retryable: true}` |
| 誤配置（`misplaced_report`） | `requires_result_file()` なら file を読む前に `result_location::misplaced_report` を呼ぶ。見つかれば `Error{retryable: false}`（`MISPLACED_RESULT_MARKER`） | 同じ | 呼ばない | 呼ばない | 呼ばない |

dispatcher 側（`dispatcher/worker_task.rs`。R1 の write_set の外）は、`requires_result_file` の run の終端が `Err(AdapterError::Other)`・`Done`・`Error` のとき、adapter の結果に関わらずもう一度 `misplaced_report` を呼び、見つかれば `Error{retryable: false}` に置き換える。したがって pi・acp・aider の誤配置も task の終端では契約違反になる（adapter の `runs/<run_id>/result.json` と `celerisctl worker run` の出力だけが「無い」と「誤配置」で違う）。

### 1b. usage と cost

| 項目 | claude_code | codex | pi | acp | aider |
|---|---|---|---|---|---|
| usage の出どころ | stream の `result.usage`（`cost_usd` は読まず `None`）。`result` を観測できない終端は `ExplorationTracker::cumulative_usage`（assistant 行の usage の累積） | `turn.completed` の usage（`cost_usd: None`） | `message_end` の usage の累積。`cost.total` があれば `cost_usd` に加算 | 常に `None`（安定版 ACP に usage の欄が無い） | stdout の `Tokens: N sent, M received.` と `Cost: … session.`（省略形 `1.2k` は捨てる） |
| cost の推定（`task_core::estimate_cost_usd`） | `usage_with_cost`: model が分かれば**常に上書き**（入力は常に `None` なので、埋めると同じ） | `with_estimated_cost`: 同じ（上書き。入力は常に `None`） | しない | しない | `usage_with_cost`: `cost_usd` が **`None` のときだけ** 埋める（aider の実測を優先） |
| 終端に usage を載せる範囲 | Done・Question・Yielded・Waiting・BudgetExhausted・Error の全部（`terminal_usage_slot`。timeout・crash も累積値を運ぶ） | Done・Question・Yielded・Waiting・BudgetExhausted{Context} は無し。Error は `None` | Done・Question・Yielded・Waiting・BudgetExhausted{WallClock}。Error は `None` | 無し | Done・Question・Yielded・Waiting。timeout・Error は `None` |

### 1c. 締め切り・cancel・log・artifact

| 項目 | claude_code | codex | pi | acp | aider |
|---|---|---|---|---|---|
| wall clock | `limits.wall_clock` 超過で `BudgetExhausted{WallClock, "wall clock exceeded", usage: None}`、process を kill | 同じ | 同じ（終端に `stream.usage` を載せる） | 同じ（`WaitOutcome::TimedOut`。`session/cancel` を送ってから kill） | 同じ |
| idle | `limits.idle_timeout` 無出力で `Error{retryable: true, "idle timeout"}`（ADR-0072 §7 U7。harness_error に分類しない） | 同じ | 同じ | 同じ | 同じ |
| 待ち時間の計算 | `min(wall_clock - elapsed, idle_timeout - idle_elapsed)` を `tokio::time::timeout` に渡し、1 行読むたびに `last_activity` を更新・`sink.heartbeat()` | 同じ | 同じ | 同じ | 同じ |
| timeout の終端処理 | `subprocess::kill_now`（process group に SIGTERM → `kill_grace` → SIGKILL） | 同じ | 同じ | `session/cancel` を stdin に書く → `kill_grace` 待つ → `send_signal_to_group(SIGKILL)`（ADR-0026 D4） | 同じ |
| 正常終了の回収 | `subprocess::reap_after_terminal` | 同じ | 同じ | 同じ | 同じ |
| cancel（dispatcher から） | `Command::process_group(0)` と `process_group::ProcessGroup::register(run_id, pid)`。止めるのは `kill_tree`（Phase 55。全経路共通） | 同じ | 同じ | 同じ | 同じ |
| log | `runs/<run_id>/stdout.jsonl`・`stderr.log`。browser の run は両方 `/dev/null` | `stdout.jsonl`・`stderr.log`。resume の RPC 失敗で fresh にやり直すときは `stdout.resume_retry.jsonl`・`stderr.resume_retry.log` | `stdout.log`・`stderr.log` | `stdout.jsonl`・`stderr.log`。browser の run は `/dev/null` | `stdout.log`・`stderr.log` |
| run dir の記録 | `request.json`・`prompt.txt`（`subprocess::write_run_request`・`write_run_prompt`）、`routing_context_transport::record`、`campaign_pin::enforce` | 同じ | 同じ | 同じ（prompt は resume の再送でもう 1 回書く） | 同じ |
| 終端の書き出し | `subprocess::write_result_json` → `runs/<run_id>/result.json`（1 行の `WorkerMessage`）。その後 `Error` と供給側失敗の組は `Err(AdapterError::from_provider_failure)` | 同じ | 同じ | 同じ | 同じ |
| artifact | `artifacts/result.json`（`req.artifact_path`）。run の先頭で前回の残りを削除、`delegate_file::clear_delegate_file`。終わりに `forward_delegate_file` | 同じ | 同じ | 同じ | 同じ |
| 起動の順 | `db_guard::launch`（container の差し込み）→ `kill_on_drop(true)` → `process_group(0)` → `subprocess::check_arg_lengths` → `spawn_after_notice`（`before_spawn` 通知の後に spawn）→ `ProcessGroup::register` → stderr を file に写す `tokio::spawn` | 同じ | 同じ | `check_arg_lengths` を呼ばない。他は同じ | 同じ |

### 1d. 観測できる食い違いのまとめ

1. 誤配置の回収を adapter で行うのは claude_code・codex だけ（dispatcher が全 adapter を覆う）。
2. cost の推定は claude_code・codex が上書き、aider は欠けたときだけ、pi・acp は無し。上書きと「欠けたときだけ」は、claude_code・codex の入力が常に `cost_usd: None` なので結果が同じ。
3. 終端に usage を載せる範囲が adapter ごとに違う（1b）。
4. stdout の log 名が `stdout.jsonl`（claude_code・codex・acp）と `stdout.log`（pi・aider）。
5. browser の run の log を `/dev/null` にするのは claude_code・acp だけ。codex・pi・aider は browser の run を扱う分岐を持たない（`context.browser` を 1 度も見ない）。
6. acp だけ `check_arg_lengths` を呼ばない。
7. acp の timeout は `session/cancel` を先に送る（wire の作法）。
8. `result_file_wait`（`adapter.rs`）が text を `Value` に parse し、その後 adapter が同じ text を `ResultFile` にもう 1 回 parse する。

## (2) 共通の契約

### D1. 結果契約の正本は `crates/task-worker/src/result_report.rs`

`result_report.rs` は既に result.json の `report`・`milestone_proposal`・`actions`・`final_message_is_recoverable_result` を読む「result.json の欄の読み手」である。終端の解釈もここに置き、file を読む関数は crate 内で 1 つにする。

- `pub(crate) struct ResultFile { summary, question, evidence: serde_json::Value, r#yield }`（名前を保つ。5 本の写しを消す）。
- `pub(crate) fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence>`（名前を保つ。5 本の写しを消す）。
- `pub(crate) fn interpret_result_text(text: &str, artifacts_rel: &str, usage: Option<Usage>) -> Terminal`: text を `Value` に 1 回だけ parse し、`question`（文字列）> `wait`（`crate::adapter::wait_terminal`）> `summary` > `yield` の順で終端を作る。JSON でない・どの欄も無い・`wait` が不正は `Error{retryable: true, usage: None}`。文言は現行のまま（`<rel>/result.json is not valid JSON: …`、`… has neither 'summary', 'question' nor 'yield'`、`result.json has an invalid cluster job wait: …`）。`question` が文字列でないときは現行どおり `ResultFile` の deserialize に失敗して「not valid JSON」になる（変えない）。
- `pub(crate) enum ResultFileRead { Misplaced(Terminal), Missing, Text(String) }` と `pub(crate) async fn read_result_file(req: &RunRequest, run_started: SystemTime) -> ResultFileRead`: `req.requires_result_file()` なら `result_location::misplaced_report(RESULT_FILE_NAME, &req.artifacts_dir, req.cwd(), run_started)` を先に呼び、見つかれば `Misplaced`。無ければ `Missing`。読めれば `Text`。`Missing` のときの終端は adapter が決める（(3)）。
- `crate::adapter::result_file_wait` は消す（呼び出し元は 5 adapter だけで、`interpret_result_text` が同じことをする）。`crate::adapter::wait_terminal(value, usage)` は `dispatcher.rs` が使うので公開の署名を保ち、`adapter.rs` に残す。

観測できる挙動の変化と理由:
- pi・acp・aider で誤配置の result.json が adapter の時点で `Error{retryable: false}`（`MISPLACED_RESULT_MARKER`）になる。task の終端は今も dispatcher（`worker_task.rs`）が同じ値に置き換えているので変わらない。変わるのは `runs/<run_id>/result.json` の文言と `celerisctl worker run` の出力。理由: ADR 2026-11-06 D1「回収は 1 つの関数」を adapter 側でも 1 か所で呼ぶため。dispatcher 側の呼び出しは write_set の外なので R1 では残す（`Err(AdapterError::Other)`・fake・R6 の adapter を覆う安全網。撤去するかは R6 で決める。(5)）。
- claude_code で、誤配置があるときの cos_chat・background task の分岐は現行どおり誤配置が勝つ（`read_result_file` を分岐の前に呼ぶ）。

### D2. 実行共通処理の正本は `crates/task-worker/src/adapter.rs`

`adapter.rs` に次を置く。adapter は自分の wire の処理の前後でこれを呼ぶ。`subprocess.rs` の既存 helper（`kill_now`・`reap_after_terminal`・`read_tail`・`read_line_limited`・`write_result_json`・`write_run_request`・`write_run_prompt`・`check_arg_lengths`・`spawn_after_notice`・`send_signal_to_group`）の署名は変えない（R6 の adapter が使う）。

- `pub(crate) struct RunPaths { run_dir, stdout_log, stderr_log, result_path, artifacts_rel }` と `pub(crate) async fn prepare_run(req, run_id) -> Result<RunPaths, AdapterError>`: run dir を作る、前回の `artifacts/result.json` を消す、`clear_delegate_file`、`campaign_pin::enforce`、`write_run_request`。browser の run は stdout・stderr の log を `/dev/null` にする。`routing_context_transport::record` は adapter が command を組んだ後に自分で呼ぶ（引数の `supported` が adapter ごとに違うため）。
- `pub(crate) fn deadline_terminal(start: Instant, last_activity: Instant, wall_clock: Duration, idle_timeout: Duration) -> Result<Duration, Terminal>`: wall clock 超過なら `Err(BudgetExhausted{WallClock, "wall clock exceeded", usage: None})`、idle 超過なら `Err(Error{retryable: true, "idle timeout", usage: None})`、それ以外は次の待ち時間 `Ok(min(..))`。`idle_timeout` を `RunLimits` でなく値で受けるのは、langmem が `[adapters.langmem].idle_timeout_secs` で `limits.idle_timeout` を縮める（ADR-0047 D4）ので、R6 がそのまま使えるようにするため。
- `pub(crate) fn spawn_capture_stderr(child: &mut Child, path: &Path, run_id: &str) -> JoinHandle<()>`: stderr を `path` に写す task（5 本で同文）。
- `pub(crate) fn with_estimated_cost(usage: Option<Usage>, model: Option<&str>) -> Option<Usage>`: `cost_usd` が `None` で model が分かるときだけ `task_core::estimate_cost_usd` で埋める（aider の規則）。claude_code・codex の `usage_with_cost`・`with_estimated_cost`、aider の `usage_with_cost` を消してこれを呼ぶ。
- `pub(crate) async fn finish_run(run_dir: &Path, terminal: Terminal, provider_failure: Option<ProviderFailure>, exit_status: ExitStatus) -> Result<RunOutcome, AdapterError>`: `write_result_json` → `Error` と供給側失敗の組は `Err(AdapterError::from_provider_failure)` → `Ok(RunOutcome)`。claude_code の `RESULT_JSON_MISSING_MARKER` を `Err(AdapterError::Other)` に倒す処理と browser の文言の伏せ字は、この関数の前後で claude_code が行う（provider 固有）。

食い違い (1d) の 1 件ずつの決定:

| 食い違い | 決定 | 観測できる変化と理由 |
|---|---|---|
| 1. 誤配置の回収 | 統一（D1 の `read_result_file`） | 上記 D1 |
| 2. cost の推定 | 統一（欠けたときだけ埋める） | claude_code・codex は入力の `cost_usd` が常に `None` なので結果は同じ（試験 `run_common_cost_estimate_keeps_reported_cost` で「実測があれば上書きしない」を固定し、`claude_code`・`codex` の stream の parse が `cost_usd: None` を置くことは既存の parse の試験で保つ）。pi は呼ばない（pi は cheap lane のローカル model（opencode-qwen 等）を含み、実測が無い＝無料の経路に静的単価表で USD を付けると予算の会計を誤る。`budget-unknown-cost-stops` の不変条件は「不明は不明のまま」を要る）。acp は usage 自体が無い |
| 3. 終端に usage を載せる範囲 | provider 固有のまま | 範囲は「その harness が usage を観測できる時点」で決まる（claude_code だけが累積値を持つ）。揃えるには codex・pi・aider に累積の観測を足す必要があり、R1 の目的（写しの集約）の外 |
| 4. stdout の log 名 | 統一しない | pi・aider の stdout は JSON 行でない（aider は平文、pi は JSON 行だが名前は `.log`）。名前を揃えるのは log を読む人・道具に見える変化で、R1 の目的の外。現行の名前を `RunPaths` が adapter の指定で作る |
| 5. browser の log の `/dev/null` | 統一（`prepare_run` が全 adapter に適用） | codex・pi・aider は browser の run を扱う分岐を持たず、browser の run は browser 専用の経路（`browser_specialist`）が claude_code・acp に渡す。これらに届かない限り観測できる変化は無い。届いたときは page の内容を harness の生の stream として残さない方が ADR-0078/0102 の方針に合う |
| 6. acp の `check_arg_lengths` | 統一しない（R1 では足さない） | acp は prompt を stdin の JSON-RPC で渡し argv に載せないので、検査の意味が無い。足すと `E2BIG` の防止ではなく無害な検査が増えるだけ |
| 7. acp の `session/cancel` | provider 固有のまま（(3)） | wire の作法 |
| 8. 二重の parse | 統一（`interpret_result_text` が 1 回） | 変化なし |

### D3. 置き換えと撤去（同じ変更で消すもの）

- 5 adapter の `struct ResultFile`・`fn lenient_evidence`・`terminal_from_result` の file 読み取りと欄の解釈の部分。claude_code の `terminal_from_result` は `ResultMeta` の判定（`is_error`・`error_max_turns`・context 超過）だけを残して `read_result_file`・`interpret_result_text` を呼ぶ。acp の `terminal_from_result_file` は `max_turn_requests` の判定と `Missing` の文言だけを残す。
- `adapter::result_file_wait`。
- claude_code・codex・aider の cost の関数 3 つ（`usage_with_cost` ×2、`with_estimated_cost`）。
- 5 adapter の run dir 準備・締め切り計算・stderr 回収・終端の書き出しの写し。
- `preamble.rs` の `pub fn worktree_note`（dup-candidates `unref-worktree-note`、決定 delete。定義の行以外に workspace で 1 度も出ず、試験からも呼ばれない。`ssh.rs` の `remote_worktree_note` は別の関数で残す）。unit run-common で消す。
- (4) で挙げる契約の写しの試験。

## (3) provider 固有として残すもの

| 区分 | claude_code | codex | pi | acp | aider |
|---|---|---|---|---|---|
| wire | stream-json の解析（`result`・`rate_limit_event`・`system` の background task・`tool_use` の写像・`ExplorationTracker`） | JSON-RPC 風の event（`thread.started`・`turn.completed`・`turn.failed`・`error`）の解析、`-c key=value` の翻訳 | `message_end`・`agent_end` の解析、Hashline extension の失敗の検出 | JSON-RPC（`initialize`・`session/new`・`session/load`・`session/prompt`・`session/cancel`）、`check_arg_lengths` を呼ばない | stdout の平文（`Tokens:`・`Cost:`）の解析 |
| 認証・account | `CLAUDE_SECURESTORAGE_CONFIG_DIR`・OAuth の cooldown | codex account | opencode go の key | ACP agent の設定 | API key |
| session の再開 | `--session-id`/`--resume`、resume 拒否の検出（stderr と `result` の両方） | `exec resume`、RPC 未実装の検出と同じ run 内の fresh やり直し（Phase 98） | 無し | `session/load` | 無し |
| result.json が無いときの分類 | `RESULT_JSON_MISSING_MARKER` → InfraRequeue。cos_chat の本文で Done。background task 残りの `Yielded`/`Error` | 「exited without」`Error{retryable: true}`。conversation・cos_chat の reply で Done。Phase 112 D3 の書き戻し | 「exited without」。cos_chat の reply で Done | 「stopped (<stopReason>) without」。cos_chat の `end_turn` で Done | 「exited without」 |
| file より前の判定 | `is_error`・`subtype`（`error_max_turns`・context 超過） | `turn.failed`（context 超過） | `stream.error`・exit 非 0・`agent_end` 無し・extension 失敗 | `max_turn_requests`・Eof・RpcError | 無し |
| timeout の止め方 | `kill_now` | `kill_now` | `kill_now`（prompt を書く task も abort） | `session/cancel` → grace → SIGKILL | `kill_now` |
| usage の観測 | stream の累積（全終端に載せる） | `turn.completed` | `message_end` の累積と実測 cost | 無し | stdout の平文 |
| browser の文言の伏せ字 | `Error`・`BudgetExhausted` の message を「browser harness failed」に置き換える | 無し | 無し | 無し | 無し |

「無いときの分類」を統一しない理由: claude_code の InfraRequeue（ADR-0072 E2）は「claude 自身が成功と申告したのに file が無い」という観測に基づく。codex・pi・acp・aider は成功の申告の強さが違い（acp は `stopReason` に成否の意味が無い。ADR-0026 D1）、同じ分類に寄せると attempts を消費しない経路が広がる。R1 では文言と分類を現行のまま adapter に残し、`Missing` を受けて adapter が終端を作る。

## (4) 試験の配置

### 4a. 共通契約の試験（`result_report.rs` の `mod tests`。5 件、名前は固定）

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `result_contract_priority_order` | `question` > `wait` > `summary` > `yield`。`{"question","summary"}` は Question、`{"wait","summary"}` は Waiting、`{"summary","yield"}` は Done、`{"yield"}` は Yielded | 既存は adapter ごとに 1〜2 組だけを fake の実行で確かめ、順位の全組を 1 か所で見る試験が無い |
| `result_contract_lenient_evidence` | 配列の要素のうち `Evidence` に読めたものだけを残し、配列でなければ空。run は失敗しない | 既存の `malformed_evidence_in_result_file_does_not_fail_the_run`（claude_code・codex）は同じ内容の写し。正本の関数を直接試す |
| `result_contract_invalid_json_is_retryable_error` | JSON でない text は `Error{retryable: true, usage: None}` で、文言に `is not valid JSON` を含む | 既存の `invalid_result_file_json_is_retryable_error`（claude_code・codex）、pi の `pi_adapter_result_question_yield_and_invalid_json` の 1 組の写し |
| `result_contract_no_known_field_is_retryable_error` | どの欄も無い JSON（`{}`・`{"evidence": []}`）は `Error{retryable: true}` で文言に `has neither` を含む。`wait` が不正なら `invalid cluster job wait` | 現行はどの adapter にも試験が無い |
| `result_contract_usage_passthrough` | 渡した usage は Done・Question・Yielded・Waiting にそのまま載り、Error には載らない（`None`） | 現行は adapter の試験が usage を見ていない |

### 4b. 実行共通処理の試験（`adapter.rs` の `mod tests`、または `adapter/tests.rs`。`run_common_` で始める。3 件以上）

| 試験 | 守る不変条件 |
|---|---|
| `run_common_deadline_wall_clock_is_budget_exhausted` | wall clock 超過は `BudgetExhausted{WallClock}`、文言 `wall clock exceeded`（dispatcher の `classify_budget_kind_from_text`・`is_budget_outcome` が見る語） |
| `run_common_deadline_idle_is_retryable_error` | idle 超過は `Error{retryable: true}`、文言 `idle timeout`。wall clock と idle の両方が超過なら wall clock が勝つ。超過が無ければ待ち時間は 2 つの残りの小さい方 |
| `run_common_cost_estimate_keeps_reported_cost` | `cost_usd` が `Some` なら上書きしない。`None` で model が分かれば静的単価表で埋める。model が無ければ `None` のまま |
| `run_common_prepare_run_clears_previous_result_and_delegate` | `prepare_run` は前回の `artifacts/result.json`・`delegate.json` を消し、`runs/<run_id>/request.json` を書く。browser の run は log の path が `/dev/null` |
| `run_common_finish_run_maps_provider_failure_to_adapter_error` | `finish_run` は `runs/<run_id>/result.json` を書いてから、`Error` と供給側失敗の組だけを `Err(AdapterError)` にする |

### 4c. adapter ごとの試験

残す（結線。fake の harness の実行で契約の関数を通ることを 1 件以上）:
- claude_code `happy_path_progress_and_done_from_result_file`、codex `happy_path_progress_and_done_from_result_file`、acp `happy_path_progress_and_done_from_result_file`、aider `a_question_in_result_json_becomes_terminal_question`（aider の唯一の file 経由の終端の試験。結線の試験として残す）、pi `pi_adapter_result_question_yield_and_invalid_json` は 1 組（question）に縮めて結線の試験にする。
- provider 固有の分類の試験は全部残す: `success_without_result_file_is_an_infra_failure_not_a_retryable_worker_error`（claude_code）、`success_without_result_file_is_retryable_error`（codex）、`missing_result_file_after_stop_reason_is_retryable_error`（acp）、`missing_result_json_is_a_retryable_error`（aider）、`stale_result_file_*`（claude_code・codex）、cos_chat の `*_done_without_result_json`・`*_stays_failed`、codex の Phase 112 の書き戻し、claude_code の background task の試験。

消す（契約の写し。4a の試験が同じ不具合を検出し、結線は上の試験が確かめる）:
- claude_code: `invalid_result_file_json_is_retryable_error`、`malformed_evidence_in_result_file_does_not_fail_the_run`（prompt に「plain strings are not accepted」が入ることの確認は、この試験の末尾から `build_prompt_includes_objective_criteria_and_result_file_instructions` に移す）、`question_in_result_file_blocks_task`、`result_yield_becomes_terminal_yielded`、`result_wait_becomes_terminal_waiting`（dispatcher の `cluster_job_wait` の試験が `Waiting` の扱いを別に固定している）。
- codex: `invalid_result_file_json_is_retryable_error`、`malformed_evidence_in_result_file_does_not_fail_the_run`、`question_in_result_file_blocks_task`、`result_yield_becomes_terminal_yielded`。
- acp: `question_in_result_file_blocks_task`、`result_yield_becomes_terminal_yielded`。
- aider: `result_yield_becomes_terminal_yielded`。
- pi: `pi_adapter_result_question_yield_and_invalid_json` の yield・invalid の 2 組。

消す理由（規約 10）: 契約（優先順位・無効値・寛容な evidence）は D1 で正本が 1 つになり、4a がその正本を直接試す。adapter の写しの試験は、正本を通る限り同じ結果を 5 回確かめるだけで、adapter 固有の結線の失敗は happy path の 1 件で検出できる（adapter は `interpret_result_text` を 1 か所で呼び、終端の種類で分岐しない）。試験関数の数は 4a・4b の 10 件を足し、写し 13 件を消すので純減 3 件。不変条件の試験は減らない。

## (5) R6（task 01M6TYVJT7EJ6WWHEF0EBH3JPS）への引き継ぎ

`langmem.rs`・`local_deep_research.rs`・`paperqa.rs` は R1 の write_set の外で R6 の write_set にある。R1 では変えず、`subprocess.rs` の helper の署名を保つ。R6 で共通化できる箇所:

1. 監視 loop: 3 本とも 5 adapter と同じ形（`start`・`last_activity`・`min(..)` の待ち・`read_line_limited`・`kill_now`/`reap_after_terminal`）。D2 の `deadline_terminal` をそのまま呼べる。langmem は `idle_timeout` を `[adapters.langmem].idle_timeout_secs` で縮めるので、引数は値で渡す（D2 で考慮済み）。paperqa は取得と pqa の 2 段で同じ `start` を使う（壁時計は run 全体）。
2. wall clock の終端: 3 本は `Error{retryable: true, "wall clock exceeded"}` で、5 adapter の `BudgetExhausted{WallClock}` と違う。dispatcher は文言の安全網（`task_core::is_budget_outcome`・`classify_budget_kind_from_text` が「wall clock」を見る）で予算切れとして拾うので task の遷移は同じだが、`runs/<run_id>/result.json` の `WorkerMessage` と `WorkerFinished` の構造化した欄が違う。R6 で `BudgetExhausted{WallClock}` に揃えるときは、研究 harness の run は continuation の対象でない（1 回の `--message` 型）ことを確かめ、`Yielded`/continuation の経路に乗らないことを試験で固定する。
3. `prepare_run`・`spawn_capture_stderr`・`finish_run`: 3 本の run dir 準備と終端の書き出しも同文。`run_subprocess`（`fake` と protocol 準拠の worker 用）も同じ loop を持つが、`WorkerMessage` を受信する側なので終端の合成は別（寄せるのは loop の締め切りだけ）。
4. dispatcher 側の `misplaced_report` の呼び出し（`worker_task.rs`）: D1 で 5 adapter が自分で呼ぶようになった後、残る役目は `Err(AdapterError::Other)` の経路と、R6 の 3 adapter・`fake`・`run_subprocess` の覆い。R6 が 3 adapter に `read_result_file` を通したら、dispatcher 側を消すか「`Err` の経路だけ」に縮めるかを決める。
5. dup-candidates の `unref-*` のうち task-worker にあるもの（R1 の write_set の外）は R6 の表のとおり。

## 代替案

- 共通の `trait ResultContract` や adapter の基底 struct を作る: 5 adapter の run は wire ごとに loop の形が違い（acp は JSON-RPC の応答待ち、pi は stdin の task）、基底 struct にすると hook だらけになる（規約 3）。関数の共有で足りる。
- `ResultFile` を `task_core` に置く: `Evidence` と `Terminal` は task-worker の protocol なので、crate をまたぐ理由が無い。
- `result_file_wait` を残す: `interpret_result_text` が同じことをするので、残すと二重実装（規約 2）。
- pi にも cost の推定を足す: 無料のローカル経路に USD を付けると予算の会計を誤る（D2 の表 2）。

## 検証（unit result-contract・run-common・record）

- `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、`bash scripts/dev/check-health.sh`。
- `grep -rn "struct ResultFile\|fn lenient_evidence" crates/task-worker/src` が `result_report.rs` の 2 行だけ。
- `grep -rn "fn result_file_wait" crates/` が 0 件。`grep -n "pub fn wait_terminal" crates/task-worker/src/adapter.rs` が 1 件。
- `grep -n "fn worktree_note" crates/task-worker/src/preamble.rs` が 0 件。
- 4a の 5 件と 4b の 3 件以上が `cargo nextest run -p task-worker -E 'test(/^result_contract_|^run_common_/)'` で通る（この filter は 0 件に当たる。付記の (h) の filter に直した）。
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` が exit 0。

## 状態の記録

- 2026-11-10: 提案。unit adr（WU `adr`）が書いた。実装の unit が終わったら「実装済み」に変え、実装との食い違いを付記する。
- 2026-11-10: 実装済み。unit result-contract（D1・D3・4a・4c）と unit run-common（D2・D3・4b）が実装し、unit record が下の付記を書いた。

## 付記（2026-11-10、実装との突き合わせ）

unit result-contract・run-common の実装（[result-contract の進捗](../progress/2026-11-10-health-v2-r1/result-contract.md)・[run-common の進捗](../progress/2026-11-10-health-v2-r1/run-common.md)）と決定節の食い違いを書く。決定（D1〜D3 の何を統一し何を provider 固有に残すか）は変えていない。

(a) D1 の型と試験の置き場。`ResultFileRead::Misplaced` は `Box<Terminal>` を持つ（clippy の `large_enum_variant`）。4a の試験は `result_report.rs` の `mod tests` ではなく、別 file `crates/task-worker/src/result_report_tests.rs`（module `result_report::result_contract_tests`）に置いた。同じ module に `read_result_file_recovers_strays_only_for_runs_that_require_the_file`（誤配置の回収を要る run だけが通す。候補 1 つは写して読む、複数は `Misplaced`、無ければ `Missing`）を足した。

(b) (1d) 8「二重の parse」の「変化なし」は誤り。`interpret_result_text` は本文を `serde_json::Value` に 1 回 parse し、`ResultFile` は `serde_json::from_value` で読む。このため serde の誤りの細部が 2 点変わった。
- 欄の型が違う本文（例 `{"question": 1}`）の文言の末尾に、serde の「at line L column C」が付かなくなった。接頭辞「`<rel>/result.json is not valid JSON: `」と `Error{retryable: true, usage: None}` は同じ。
- 同じ鍵が 2 回ある本文（例 `{"summary":"a","summary":"b"}`）は、「duplicate field」の `Error` ではなく後の値（`"b"`）で読む。`Value` の map が後の値で上書きするため。

(c) D1 の「観測できる挙動の変化」は 2 点で書き足す。
- pi・acp・aider が `read_result_file` に渡す時刻は run の開始ではなく、監視 loop の開始（spawn の後の `start: Instant`）を壁時計に直した値（`SystemTime::now() - start.elapsed()`）。誤配置の候補の窓はそこから 2 秒前まで。worker が result.json を書くのは spawn の後なので、候補から落ちる file は無い。`run_pi`・`run_acp`・`run_aider` は base の時点で 200 行を超えていて、`let run_started = …` の 1 行でも repo-health の `size.responsibility-growth` が fail になったため。claude_code・codex は run の開始のまま。
- D1 は「task の終端は変わらない」と書いたが、誤配置の候補が 1 つのときは変わる。これまで pi・acp・aider の adapter は「exited without …」の `Error{retryable: true}` を返し、dispatcher（`worker_task.rs`）の `misplaced_report` は置き場に写すだけで `None` を返すので、終端は `Error` のまま再試行していた。今は claude_code・codex と同じく写した本文で終端を作る（例 `Done`）。候補が複数のときは従来どおり `Error{retryable: false}`（`MISPLACED_RESULT_MARKER`）。

(d) D2 の置き場と署名。
- `RunPaths`・`prepare_run`・`finish_run` は `adapter.rs` ではなく `crates/task-worker/src/subprocess.rs` に置いた。`adapter.rs` に置くと `adapter` が `campaign_pin`・`delegate_file`・`subprocess` を呼び、3 つとも `adapter` の型に依存するので、repo-health の `deps.cycle` が base の 3 単位（adapter・tool_policy・progress）から 6 単位に増えて fail した。`subprocess` は既に `adapter`・`campaign_pin` に依存し、`subprocess` に依存し返す module が無いので、循環は増えない。`deadline_terminal`・`spawn_capture_stderr`・`with_estimated_cost` は `adapter.rs` にある。
- `deadline_terminal` は `Result<Duration, Box<Terminal>>` を返す（clippy の `result_large_err`）。
- `spawn_capture_stderr` は `Result<JoinHandle<()>, AdapterError>` を返す（stderr が pipe でないときの `AdapterError::Other("worker stderr was not piped")` を保つ）。stderr の写しの失敗の warn に `run <run_id>:` が付いた。
- `prepare_run(req, run_id, stdout_name)` は stdout の log 名を引数で受ける（D2 の表 4）。
- acp の private な `finish_run` は `finish_session` に改名し、最後に `subprocess::finish_run` を呼ぶ。

(e) D2 に書いていない観測できる変化: run の準備の順。`campaign_pin::enforce` と `request.json` の書き出しが、prompt を組む前に動く（codex・pi・acp では必須規則 `required_rules::preamble_for_run` の読み取りの前、claude_code では画像の入力の組み立ての前、pi では skill の配送の前）。必須規則が読めずに run が落ちたときも `request.json` は残る。両方が失敗する run では、返る誤りが campaign の pin の方になる。

(f) 4b の試験の置き場。`run_common_deadline_wall_clock_is_budget_exhausted`・`run_common_deadline_idle_is_retryable_error`・`run_common_cost_estimate_keeps_reported_cost` は `crates/task-worker/src/adapter_tests.rs`（module `adapter::run_common_tests`）、`run_common_prepare_run_clears_previous_result_and_delegate`・`run_common_finish_run_maps_provider_failure_to_adapter_error` は `crates/task-worker/src/subprocess/tests.rs`（(d) の置き場に合わせた）。

(g) (4) の試験の数。決定は「10 件を足し、写し 13 件を消して純減 3 件」だったが、実際は 11 件を足した（4a の 5 件、(a) の `read_result_file_…` 1 件、4b の 5 件）。消した試験関数は 12 件（claude_code 5・codex 4・acp 2・aider 1）。pi の `pi_adapter_result_question_yield_and_invalid_json` は消さずに question の 1 組に縮め、`pi_adapter_result_file_question_becomes_terminal_question` に改名した（決定の 13 件はこの関数を数えていた）。`crates/task-worker/src` の `#[test]`・`#[tokio::test]` は base の 1192 から 1191 になった（純減 1 件）。

(h) 検証節の nextest の filter `test(/^result_contract_|^run_common_/)` は 0 件に当たる（`cargo nextest list -p task-worker -E …` で確かめた）。nextest の試験名は `result_report::result_contract_tests::result_contract_priority_order` のように module の path を含むので、`^` で始まる正規表現は当たらない。正しい filter は `test(/::(result_contract|run_common)_[a-z0-9_]+$/)` で、10 件（4a の 5 件と 4b の 5 件）に当たり、10 件とも通る。module 名 `result_contract_tests` にも当たる `test(/run_common_|result_contract_/)` は、(a) の `read_result_file_…` を含めて 11 件になる。
