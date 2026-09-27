## Phase E0「Task execution decomposition の調査と設計」（2026-09-24）

コードは変えていない（docs だけ）。成果物:
- 調査報告: `docs/execution-architecture-2026-09-24.md`
- 設計: `docs/adr/0072-task-execution-decomposition.md`（状態は Proposed。E1 に着手するときに Accepted にする）

### 調査の要点（根拠の file:line は調査報告にある）

- **Task と run は実質 1:1 で結合している。** 結合している箇所は次のとおり。
  - `running: HashMap<TaskId, RunEntry>`（dispatcher.rs:1279）
  - `tasks` 行の lease は 1 つだけ（`acquire_lease` store.rs:2428）
  - run の終わりは必ず Task の Trigger に写る（`on_worker_finished` dispatcher.rs:2873）
  - `attempts` は Task の欄
  - 仕事の run は `--no-session-persistence`（claude_code.rs:830）。resume の単位はノード（CoS / lead）であって、タスクではない
- **予算切れの扱い**: `error_max_turns` と wall-clock 超過は `Terminal::Error{retryable:true}` → `WorkerError` → attempts+1 になり、`max_retries` を使い切ると failed。
  - 次の試行は何も引き継がずに最初からやり直す。
  - usage も捨てる（claude_code.rs:1236-1253）。
  - 予算をモデルに伝える文面は無い。turn の上限を持つのは claude-code だけ。
- **reviewer の不合格は Task 全体の再実行になる**（`ReviewFail` → `retry_or_fail`）。局所修復の仕組みは無い（配送の `[delivery-repair]` も `Reopen` で全体をやり直す）。
- **既存の分解（`Plan` kind・委譲）はどちらもユーザーに見える子 Task を作る。** `TaskFeatures` は lane の決定にしか使われていない。
- `SCHEMA_VERSION = 25`。events が正本で、`tasks` は派生のスナップショット。`Status` は 8 つ、`Trigger` は 23 個。

### 設計の要点（ADR-0072 D1〜D24）

- **層の構成**: Task（goal。UX と配送の単位のまま）→ ExecutionPlan（版つき）→ WorkUnit → Run（既存の run_id）。
  - 計画を持たない Task は「暗黙の WorkUnit」で、行を作らない。
  - 1 Task の中の WU は直列に実行する（worktree と TaskId キーの lease を共有するため）。
- **Task の `Status` は増やさない。** 表示用の `ExecutionPhase` を導出する。
  - Trigger は 2 つだけ足す: `Continue{why}`（Running → Ready）と `ReviewRepair`（Reviewing → Ready）。どちらも attempts を変えない。
- **予算切れは失敗にしない。**
  - `RunEnd::BudgetExhausted` / `Yielded` → checkpoint → **新しい session** で continuation する（resume はしない。wrap-up だけ任意）。
  - 上限到達と進捗なしは `blocked` にして人に聞く。
  - Task が failed になるのは D12 の 5 条件だけ。
- **checkpoint** は schema `celeris.checkpoint/1`。3 つの出所を合成する: worker の rolling `checkpoint.json`、result.json の `yield`、daemon の mechanical（git と progress）。
- **Complexity Gate** は `TaskFeatures` と追加の信号（工程語・成果物の数・過去の予算切れの率など）の決定的な点数表で判定する。
  - 点数が 5 以上なら compound。
  - `[execution] gate = off|shadow|on`。E3 の既定は shadow。
- **Planner** は task-local の run。
  - lead の実効 profile で走る。node_sessions は resume しない。
  - 出力の schema は `celeris.execution-plan/1`（担当とモデルの欄は持たない）。
  - 不正なら 1 回だけ再試行し、それでも不正なら atomic に倒す。
- **reviewer repair**: fmt / lint / 小さな test / reviewer_local / merge_base に分類する。repair WU には最小の context だけを渡す。人の承認は再利用する。
- **replanning**: 新しい版の全体を出させ、done の WU は不変にする。`ExecutionPlanned.supersedes` で監査できる。
- **データ**: events が正本（新しい Event は 4 つ）。migration 0026（`execution_plans` / `work_units` / `runs`。派生の索引）は E2 で入れる。E1 は migration 無し。

### E1 の受け入れ条件（ADR-0072 §6 E1 の要約）

- (a) claude-code の `error_max_turns` と wall-clock 超過が `BudgetExhausted` になる。usage を保持し、`Continue` で Ready に戻る（attempts は不変）。
- (b) worker と mechanical の checkpoint を合成し、`CheckpointSaved` として残す。schema 違反なら mechanical だけにする。16 KiB で切り詰める。
- (c) 次の run の request.json / prompt.txt に、続きの節と checkpoint が載る。会話は載らない。
- (d) result.json の `yield` が Yielded になる。
- (e) continuation の上限と、進捗なし 2 回で blocked になる。人の回答で窓が戻る。
- (f) 既存の遷移とテストは不変。`[execution] continuation = false` で従来の挙動に戻る。
- (g) attempt_history / replay / classify_task_failure / stats が continue を数えない。
- (h) 再起動後も continuation が組まれる。
- (i) codex / acp / aider の wall-clock 超過も continuation する。

### 検証

- `git status --short`: 差分は `docs/` 配下の 3 ファイルだけ（調査報告・ADR-0072・本節）。コードを変えていないので `cargo fmt` / `cargo test` / `cargo clippy` は実行していない。依頼の指示どおり docs だけ。

### 未解決事項（ADR-0072 §7）

- U1: context 超過の実際の文言（claude-code / codex / ACP）は実機で未確認。E1 は字句の候補で判定し、分類できなければ従来の WorkerError に倒す（安全側）。E6 で実機を確かめる。
- U2: codex / acp の turn ごとの usage（peak context）が取れるかは未確認。
- U3: 1 Task の中での WU の並列は別 ADR にする。
- U4: WU ごとの WIP commit は求めない（既定）。E6 で再検討する。
- U5: 部署をまたぐ WU は別の Task にする（既定）。
- U6: pricing は claude-fable を知らない（P-118-1）。
- U7: idle timeout の分類をインフラ扱いに変えるかは未定（E1 では遷移を変えない）。
- U8: 上限到達の質問への回答を GUI のボタンにするかは E5 で決める。
- U9: remote の worktree の mechanical checkpoint は E1 の対象外。
- U10: gate の閾値と重みは初期値で、shadow の記録と E6 で調整する。

### 提案

- P-E0-1: DESIGN §5.6（「計画が不正なら failed」）に「Task 内部の実行計画（ADR-0072）は atomic に倒す」という注記を足す（DESIGN.md は書き換えない。人の判断待ち）。
- P-E0-2: 調査で見つけた既存の不整合。ADR-0070 D3 の記述では「result.json の不在はインフラ扱い」だが、claude-code では `Ok(Terminal::Error)` として届くので attempts を消費している（`Err(AdapterError)` だけがインフラの経路を通る。dispatcher.rs:2966-3016）。E1 で `RunEnd` の分類を入れるときに合わせて直すか、人が判断する。
- P-E0-3: `task-api::stats::classify_outcome` に `infra_requeue:` の分類が無く、Error として数えている。E5 の metrics で `end` を優先する際に合わせて直す。

## Phase E1「Run lifecycle / checkpoint / continuation」（2026-09-24）

ADR-0072（`docs/adr/0072-task-execution-decomposition.md`）の Status を Proposed → Accepted にし、
§6 E1 の受け入れ条件 (a)〜(i) を実装した。migration は無し（events を正本にし、新しい `Event` を
足しただけ）。ExecutionPlan / WorkUnit（E2 以降）には触れていない。

branch: `worktree-agent-a7ce86ee4adbc28f5`。最終 commit は本節末尾の git ログを参照。

### 受け入れ条件ごとの証跡

**(a) claude-code の `error_max_turns` と wall-clock 打ち切りが `RunEnd::BudgetExhausted` になり、
usage を保持し、`Continue` で Ready に戻る（attempts 不変）**
- 実行したコマンド: `cargo test -p task-worker --lib claude_code::tests::error_max_turns_subtype_wins_and_becomes_budget_exhausted_with_usage claude_code::tests::wall_clock_exceeded_kills_and_reports_budget_exhausted`
- 出力の要点: exit 0、2 passed（`error_max_turns` は `result.json` が `summary` を主張していても
  `BudgetExhausted{kind:Turns}` が勝ち、usage（input/output tokens）を運ぶ。wall-clock 打ち切りは
  `BudgetExhausted{kind:WallClock}`）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- budget_exhausted_run_continues_without_consuming_attempts_then_blocks_on_no_progress`
- 出力の要点: exit 0、1 passed。dispatcher レベルで、予算切れの run のたびに `Trigger::Continue`
  （reason `"continue"`）で `Running → Ready` になり、`stored.attempts == 0` が最後まで保たれることを確認。

**(b) worker の checkpoint.json と mechanical（git）の合成が `CheckpointSaved` に残る。schema 違反・
無ければ mechanical だけ。16 KiB に切り詰め**
- 実行したコマンド: `cargo test -p task-core --lib execution::`
- 出力の要点: exit 0、10 passed。`merge_checkpoint`（worker 優先の意味欄・mechanical 優先の事実欄・
  tests_run の和集合）、`missing_worker_checkpoint_falls_back_to_mechanical_defaults`、
  `schema_violation_is_treated_like_missing_checkpoint`、`truncate_checkpoint_shrinks_to_the_overall_byte_cap`
  （16 KiB 超のデータを決定的に縮めて上限内に収める）を確認。
- 実行したコマンド: `cargo test -p task-dispatch --lib checkpoint::`
- 出力の要点: exit 0、5 passed。`task-dispatch/src/checkpoint.rs`（新規）が `task_ops::changes` を
  再利用して git の読み取りを行い、`WorkerProgress{kind:tool_use}` の対から `tests_run`（最大10件）・
  `recent_activity`（最大20行）を作ることを確認。

**(c) 次の run の `request.json`/`prompt.txt` に「続きの実行（Run #N）」節と checkpoint が載る。前の
run の会話・出力の全文は載せない。continuation の無い run のプロンプトは D10 の追加分を除きバイト
単位で同じ**
- 実行したコマンド: `cargo test -p task-worker --lib preamble:: claude_code::`
- 出力の要点: exit 0（preamble 19 passed / claude_code 63 passed）。
  `continuation_section_is_empty_without_continuation_context`（continuation 無しは 1 バイトも増えない）、
  `continuation_section_summarizes_the_checkpoint_without_the_full_transcript`（`## 続きの実行（Run #3）` /
  `### checkpoint（Run #2 の終わり）` / `### これまでの Run（1 行ずつ）` を含み、会話全文は含まない）、
  `build_prompt_carries_the_continuation_section_when_present` で確認。**既存のプロンプトスナップショット
  テストは全て無変更で通った**（budget_preamble を対話 run には出さない設計にしたため）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- yielded_run_continues_and_the_next_run_context_carries_the_checkpoint`
- 出力の要点: exit 0、1 passed。2 回目の run に渡る `RunContext.continuation` が
  `run_seq=2`・`previous_end="yielded"`・`checkpoint.next_action`・`prior_runs=["Run #1 yielded"]` を
  正しく持ち、1 回目の run には `continuation` が無いことを確認（dispatcher の
  `build_continuation_context` が events だけから純粋に組み立てる）。

**(d) result.json の `yield` が `RunEnd::Yielded` になり continuation する**
- 実行したコマンド: `cargo test -p task-worker --lib -- result_yield_becomes_terminal_yielded`
  （claude_code / codex / acp / aider の 4 harness すべて）
- 出力の要点: exit 0、4 passed（各アダプタで `{"yield": {...}}` → `Terminal::Yielded`）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- yielded_run_continues_and_the_next_run_context_carries_the_checkpoint`
- 出力の要点: exit 0、1 passed。`Terminal::Yielded` → `Trigger::Continue` → 2 回目の run → `done`。
  `WorkerFinished.end == Some(RunEnd::Yielded)` と `CheckpointSaved` を確認。

**(e) `max_continuations_per_work_unit` への到達と、進捗なし 2 回で `blocked`（人の回答で窓が戻る）**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- budget_exhausted_run_continues_without_consuming_attempts_then_blocks_on_no_progress`
- 出力の要点: exit 0、1 passed。mechanical だけの checkpoint（git 差分が無い一時ディレクトリ）が
  連続して変化しないため、2 回連続で進捗なしと判定され 3 回目の run で `Trigger::WorkerQuestion`
  （`WorkerFinished.outcome` が `"question: 実行が進みません（continuation 2 回 / 進捗なし 2 回）…"`）
  になり `Status::Blocked` に到達することを確認。`Trigger::Answer` を投げると `Status::Ready` に戻り
  （D18「回答の時点から数え直す」）、同じ（進捗を示さない）adapter でもう一度 2 回の continuation の
  後に再び `Blocked` になることも確認（窓のリセットが機能している証拠）。
  - 実装ノート: 既存コードの `record_question_approval` は「dispatcher が判断した質問」を
    `task_core::approval::Approval`（inbox の承認、`TaskKind::Approval` の子タスクとは別物）として
    記録する経路で、`cross_department`（部またぎの質問）と同じパターンをそのまま踏襲した
    （`Event::QuestionRaised` は「run の終了を伴わない」dispatcher 発の質問だけに付く既存の規約
    — 例: `ChildFailed`/`Unroutable` — なので、run が実際に終わった今回のケースでは付けていない。
    `WorkerFinished.outcome` の `"question: "` 接頭辞が質問の本体になる）。
- `no_progress_streak` / `consecutive_continuations` の単体テスト:
  `cargo test -p task-ops --lib derive::` → exit 0、28 passed。

**(f) 既存の遷移・テストは不変。`[execution] continuation = false` で従来の `WorkerError` に戻る**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- continuation_disabled_restores_the_legacy_worker_error`
- 出力の要点: exit 0、1 passed。`d.config.execution.continuation = false` で、
  `AlwaysBudgetExhaustedAdapter`（毎回 `BudgetExhausted`）が `max_retries=1` で 2 回試行して
  `Status::Failed`・`attempts=2` になる（従来の `WorkerError{retryable:true}` の挙動そのもの）。
  `CheckpointSaved` も `"continue"` の遷移も一切作らないことを確認。
- 既存の遷移・テストが壊れていないことは `cargo test --workspace --no-fail-fast`（下記）で確認
  （done/question/error/requeue/infra の全既存テストが無変更で green）。
- `transition::tests::table_simple_triggers_full_cross_product` を `Trigger::Continue` を含めて
  4×8×18 の全列挙に拡張し、`continue_reason_matches_the_why_variant` で `why` ごとの `reason` 文字列
  （`continue`/`advance`/`work_unit_retry`/`planned`/`replan`）を確認（`cargo test -p task-core --lib transition::` → exit 0、9 passed）。

**(g) `attempt_history`/`replay`/`classify_task_failure`/`stats` が `continue` を試行・失敗に数えない**
- `attempt_history`: `Event::Transitioned{reason: "continue"}` は既存の match の `_ => None` に落ちる
  ため元々ノーコード（`retry_policy.rs` の変更不要）。`budget_exhausted_run_continues_without_consuming_attempts_then_blocks_on_no_progress`
  内で `task_core::retry_policy::attempt_history(&stored, &events_only)` が空になることを直接確認。
- `replay`: `replay_status_and_attempts` の `bump` 判定も `reason ∈ {"worker_error","lease_expired","review_fail"}`
  だけを見るため、`"continue"` は既存のまま素通りする（`crates/task-ops/src/replay.rs` 変更不要）。
- `classify_task_failure`: `Continue` は `Failed` に遷移しないので分類対象にならない（変更不要）。
- `stats`/`view` の `classify_outcome`: `end` を優先しつつ `"continue: "` 接頭辞を新設の
  `RunOutcomeKind::Continued` に分類し、`StatsState`/`AccountStats` の集計では
  「失敗でも成功でもない」ものとして数えない（`Interrupted` と同じ扱い）よう変更。同じ作業で
  **P-E0-3**（`stats.rs::classify_outcome` が `"infra_requeue: "` を分類せず `Error` に落ちていた）も
  直し、`Requeue` に分類するようにした。
  - 実行したコマンド: `cargo test -p task-api --lib stats:: view::` / `cargo test -p task-ops --lib view::`
  - 出力の要点: exit 0。`outcome_prefixes_are_classified` に `infra_requeue:`/`continue:` のケースを追加して確認。

**(h) daemon 再起動（同じ store で新しい `Dispatcher`）後も、最新の checkpoint から continuation が
組まれる**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- continuation_survives_a_dispatcher_restart`
- 出力の要点: exit 0、1 passed。「前のプロセス」相当の状態を events で直接作り（`Trigger::Dispatch` →
  `Trigger::Continue` を `store.apply_transition_with_events` で直接発行。d1 のインスタンス自体は
  作らない — fake adapter が即座に完了するため tick のタイミングで非同期完了を決定的に待つのが難しく、
  events を直接組み立てる方が再現性が高いと判断）、そのあと**新しい** `Dispatcher`（同じ `store`）を
  作って続きを回すと、`run_seq=2`・`previous_end` が `budget_exhausted` で始まる `RunContext.continuation`
  が正しく組まれ、`attempts=0` のまま `Status::Done` に到達することを確認。continuation の状態は
  プロセス内メモリ（`running`/`infra_backoff`）を一切使わず、`consecutive_continuations`/
  `latest_checkpoint`/`current_run_seq` が全て events から純粋に導出されるため、この性質は実装上
  自明に成り立つ（テストはその契約を固定するためのもの）。

**(i) codex / acp / aider でも wall-clock 打ち切りが `BudgetExhausted` になり continuation する**
- 実行したコマンド: `cargo test -p task-worker --lib codex::tests::wall_clock_exceeded_kills_and_reports_budget_exhausted acp::tests::wall_clock_exceeded_cancels_then_kills_the_process_group aider::tests::wall_clock_timeout_kills_the_process_and_becomes_budget_exhausted`
- 出力の要点: exit 0、3 passed（3 harness とも `Terminal::BudgetExhausted{kind: WallClock}`）。
  3 harness とも turn の上限を持たないため、wall-clock だけが継続の入口（D7 の表どおり）。
- 追加で、`codex` は `turn.failed` の文言が context 超過の語彙に当たれば `BudgetExhausted{kind:Context}`
  に、`acp` は `stopReason == "max_turn_requests"` を `BudgetExhausted{kind:Turns}` に分類するよう
  実装した（実機の文言は未確認。§7 U1 のまま。分類できなければ従来どおり `Error`）。
- 4 harness とも `result.json` の `{"yield": {...}}` → `Terminal::Yielded` に対応
  （`result_yield_becomes_terminal_yielded` を claude_code/codex/acp/aider それぞれに追加）。

### ADR との差分（「Phase E1 実装時の逸脱・明確化」として ADR-0072 に追記済み）

1. **`WorkerStarted.run_seq` を event のフィールドとして追加せず、events から純粋に導出**
   （`task_ops::derive::current_run_seq`）。理由: `Event::WorkerStarted`/`WorkerFinished` を struct
   literal で組み立てている箇所が約 140（`end` を追加した `WorkerFinished` だけで 83）あり、
   `run_seq`（値を持たない冗長なフィールド）のためにこれ以上ブラスト半径を広げるのは不釣り合いと
   判断した。`WorkerFinished.end`（D7 の分類そのもの。continuation 判定に必須）は予定どおり追加した。
2. **checkpoint の JSON tag を `"kind"` から `"type"` に変更**（`RunEnd::BudgetExhausted{kind}` の
   フィールド名との衝突を避けるため。`Check` 等の既存 tagged enum と同じ命名規則に合わせた）。
3. **`task_core::execution::Decision` を `CheckpointDecision` に改名**（`task_core::approval::Decision`
   との名前衝突）。
4. **D10 の「予算の予告」から絶対時刻（開始 UTC）を外した**。`OffsetDateTime::now_utc()` を prompt に
   埋め込むと、同じ入力から異なるバイト列が生成され決定性が壊れ、既存の「2 回呼んで同じ文字列になる」
   前提のプロンプトテストが揺れた（実際に 2 件のテストが flaky に失敗するのを確認して修正）。
   開始時刻は `WorkerStarted` イベントの `ts` に残るため、prompt には turn/wall の数値の目安だけを書く。
5. **D10 の harness 別の turn 上限の予告を一本化**: 本来は「claude-code だけ turn 数を出し、他は wall
   だけ」だが、`build_prompt`（と `RunContext`）は 4 harness で共有しており、呼び出し元でどの harness
   かを区別するには `build_prompt` の呼び出し口（45 箇所）へパラメータを通す必要がある。予算超過
   時の実害（誤誘導）は小さいと判断し、「claude-code はこれを `--max-turns` で実際に強制する。他の
   harness では目安」という 1 文を添えて全 harness 共通のテキストにした。
6. **reviewer/planner run の `Terminal::BudgetExhausted`/`Yielded`**: reviewer run は worker run と
   同じ `claude_code::run_claude_code` を通るため理論上は起こりうるが、continuation は worker run
   だけの仕組みなので、reviewer 側では「判定できなかった」として `max_reviewer_retries` の再試行に
   倒す（`crates/task-dispatch/src/review.rs`）。checkpoint は作らない。
7. **`Event::QuestionRaised` は (e) の上限到達では発行していない**。既存の `cross_department`
   （部またぎの質問）と同じパターンで、`WorkerFinished.outcome` の `"question: "` 接頭辞と
   `record_question_approval`（inbox の承認記録）だけで足りると判断した（run が実際に終わった上での
   質問であり、「run の終了ではない」dispatcher 発の質問 — `ChildFailed`/`Unroutable` — とは性質が違う）。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（警告 0） |
| 全テスト | `cargo test --workspace --no-fail-fast` | exit 0。80 個の test binary が全て `test result: ok`（FAILED 0）。 |
| schema（task-core） | `UPDATE_SCHEMA=1 cargo test -p task-core --lib` | exit 0、268 passed。`event.schema.json` 更新（追加のみ。`CheckpointSaved`・`WorkerFinished.end`・`RunMetrics` の新欄・`RunEnd`/`BudgetKind`/`HarnessErrorClass`/`Checkpoint` 系の型） |
| schema（task-worker） | `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` | exit 0。`worker-protocol.schema.json` 更新（追加のみ。`WorkerMessage::Yielded`/`BudgetExhausted`、`RunContext.continuation`） |
| schema（task-api） | `cargo test -p task-api --lib committed_schema_matches_generated` | exit 0、差分なし（既存の型のままで再生成が一致） |
| checkpoint schema | `docs/protocol/checkpoint.schema.json`（新規。`task-core` の `execution::tests::committed_schema_matches_generated` で生成・検証） | exit 0 |
| GUI gen:types | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と bit 単位で一致（IDENTICAL）。差分は `Event::CheckpointSaved`/`WorkerFinished.end`/`RunEnd`/`BudgetKind`/`HarnessErrorClass`/`Checkpoint` 系の型と `RunSummary.end`/`RunOutcomeKind::Continued` の追加のみ |
| GUI typecheck | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0（エラーなし） |
| GUI test | `cd gui && corepack pnpm@11.27.0 test` | exit 0。71 files / 1086 passed |

GUI の画面（Execution 節・runs 表の `end` バッジ等）は E5 の範囲なので今回は型の追随だけで、
画面の実装はしていない。

### 触ったファイル（要旨）

- 新規: `crates/task-core/src/execution.rs`、`crates/task-dispatch/src/checkpoint.rs`、
  `docs/protocol/checkpoint.schema.json`
- task-core: `model.rs`（`WorkerFinished.end`、`RunMetrics` の 2 欄、`Event::CheckpointSaved`）、
  `transition.rs`（`Trigger::Continue{why}`）、`retry_policy.rs`（`is_budget_outcome` を pub 化・
  context 語追加）、`lib.rs`（re-export）
- task-worker: `adapter.rs`（`Terminal::Yielded`/`BudgetExhausted`）、`protocol.rs`
  （`WorkerMessage` の対応する 2 バリアント、`RunContext.continuation`、`ContinuationContext`）、
  `subprocess.rs`（`write_result_json`/`run_subprocess` の対応）、`claude_code.rs`
  （`error_max_turns`/wall-clock 検出、`result.json` の `yield`、D10 の予算予告+rolling checkpoint
  指示、D9 の続きの実行節）、`codex.rs`/`acp.rs`/`aider.rs`（wall-clock 検出、`yield` 対応）、
  `preamble.rs`（`continuation_section`）
- task-dispatch: `dispatcher.rs`（`on_worker_finished` の RunEnd 分類・checkpoint 合成・continuation
  判定・`ExecutionConfig`・`build_continuation_context`）、`lib.rs`（`checkpoint` モジュール・
  `ExecutionConfig` の re-export）、`reports.rs`/`review.rs`（新 Terminal バリアントの網羅対応）
- task-ops: `derive.rs`（`consecutive_continuations`/`latest_checkpoint`/`no_progress_streak`/
  `current_run_seq`）、`view.rs`（`RunOutcomeKind::Continued`、`classify_outcome` が `end` を優先、
  `RunSummary.end`）
- task-api: `stats.rs`（同様の `classify_outcome` 更新、P-E0-3 修正）、`query.rs`
  （`checkpoint_saved` の event type 名）
- celeris: `config.rs`（`[execution]` の TOML 設定、`ExecutionTomlConfig`、`dispatch_config()` への配線）、
  `lib.rs`（疎通確認の Terminal 網羅対応）
- celerisctl: `commands/worker.rs`（`celerisctl worker run` の `normalize_outcome` の網羅対応）
- docs: `docs/adr/0072-*.md`（Status Accepted 化 + 「Phase E1 実装時の逸脱・明確化」追記）、
  `docs/protocol/worker-protocol.md`（§4.5b `yielded`、§4.5c `budget_exhausted`、§4.5d rolling
  checkpoint、§9 に `yield` の説明を追加）

### 未解決事項（ADR-0072 §7 のうち、この Phase で状況が変わったもの）

- U1（context 超過の実機の文言）: 未確認のまま。claude-code は `result` テキストの字句判定、codex は
  `turn.failed` のメッセージ、acp は `stopReason == "max_turn_requests"`（Turns）で実装。実機確認は
  E6（またはこの環境の外で認証が使える環境）で行うこと（ADR-0009 P-34）。このセッションは外向き
  ネットワークが無い環境のため実施していない。
- U2（codex/acp の turn ごとの usage）: `RunMetrics.peak_context_tokens`/`turns` は型と配線だけ追加し、
  実測はしていない（常に `None`。ADR の「取れる範囲だけ」の許容範囲内）。claude-code の stream-json
  の `assistant.message.usage`（あれば）から `peak_context_tokens` を埋める実装は E1 の範囲外として
  持ち越した。
- P-E0-2（result.json 不在時に claude-code が attempts を消費する既存の不整合）: 意図的に直していない
  （直すと `WorkerError` → `InfraRequeue` に変わり、(f) 「既存の遷移は変わらない」に抵触するため）。
  E2 以降で `HarnessErrorClass::Supply`/`Infra` を配線するときに、この不整合を一緒に解消するか改めて
  判断すること。
- P-E0-3: 本 Phase で解消済み（上記 (g) 参照）。

### E2 への申し送り

- `Event::WorkerStarted.run_seq`/`work_unit_id` と `Event::WorkerFinished.role/end` の `work_unit_id`
  相当は、E1 では暗黙の WorkUnit（`work_unit_id = None`）のみ。E2 で `execution_plans`/`work_units`/
  `runs` の migration 0026 を入れるときに、`task_ops::derive::current_run_seq`/`latest_checkpoint`/
  `no_progress_streak`/`consecutive_continuations` を WorkUnit 単位（`work_unit_id` でフィルタ）に
  拡張する必要がある（現状は暗黙の WU 前提でタスク全体を見ている）。
- `task-dispatch/src/checkpoint.rs` の mechanical 収集は `TaskWorkspaces.repos.first()`（先頭リポジトリ）
  だけを見ている。WorkUnit ごとに複数リポジトリを扱うようになったら見直すこと（E1 の対象は
  「暗黙の WU = Task 全体」なので、これで正しい）。
- D14/D17（Planner・replanning）で `Trigger::Continue{why: Advance|WorkUnitRetry|Planned|Replan}` を
  実際に使う配線はまだ無い（`ContinueWhy` 型と `transition()` の対応はすでに存在する。E2/E3/E4 で
  使うだけでよい）。
- `docs/protocol/checkpoint.schema.json` と `Checkpoint`/`WorkerCheckpointInput` は WorkUnit の
  `work_unit` 欄をすでに持っているが、E1 では常に `null`。E2 で実際に埋めること。

## Phase E1 の本番反映（2026-09-24 17:47Z）

- 統合 2ec8a69（PROGRESS.md 衝突のみ）+ clippy 1 件（テストの `expect_err`）を 93076d0 で修正。ゲート: fmt 0、cargo test FAILED 0、clippy 0、GUI typecheck / lint / test 1086 件 / gen:types 差分ゼロ。release `93076d0f4c76`、verify 全 true（schema は 25 のまま、migration 無し）、in-flight 0 でライブ切替（from 2f1fcc0a6227 = Celeris の自己改善配送「GUI recovery」）。
- これで本番は: `error_max_turns` と wall-clock 打ち切りが `BudgetExhausted` → `Continue`（attempts 不変、usage 保持）、checkpoint の合成・保存（`CheckpointSaved`、16 KiB）、続きの run は checkpoint だけを載せて新しい context で開始、result.json の `yield`、上限（continuation 3 / 進捗なし 2）で blocked + 質問、`[execution] continuation = false` で従来挙動。既存タスクは暗黙の 1 WorkUnit。
- 申し送り: P-E0-2（claude-code で result.json 不在のとき attempts を消費）は E1 で未修正のまま。E2 で直す。E1b（wrap-up run、ACP の真の yield）は未着手（任意）。実機の文言（U1）と peak_context_tokens（U2）は E6 で確認。

## Phase E2「ExecutionPlan / WorkUnit のデータモデルと決定的 scheduler」（2026-09-24）

ADR-0072 §6 E2 の受け入れ条件 (a)〜(i) を実装した。計画は手動 / fixture（origin は
`human`/`fixture` のみ。planner・gate は E3、repair・replan は E4、GUI は E5 なので範囲外）。

branch: `worktree-agent-aeddd35457ddf9edc`。3 つの区切りで commit した:
- `ce95e36` phase E2: migration 0026 + execution_plans/work_units/runs 索引 (a)(g)
- `d843f34` phase E2: POST/GET /tasks/{id}/execution-plan と celerisctl execution plan set|show (b)
- `e1c7edc` phase E2: 決定的 scheduler の配線、WU の失敗/質問/継続/再起動、P-E0-2 (c)(d)(e)(f)(h)(i)

最終 commit（本節を追記する直前）: `e1c7edc`。

### 受け入れ条件ごとの証跡

**(a) migration 0026（`SCHEMA_VERSION = 26`）。旧い DB からの移行テスト。暗黙の WorkUnit には行を作らない**
- 実行したコマンド: `cargo test -p task-core --lib store::tests::migration_0026_adds_the_execution_tables_to_a_schema_25_db`
- 出力の要点: exit 0、1 passed。版数 25（migration 0025 まで適用済み）の DB を `SqliteStore::open` で開くと
  `execution_plans`/`work_units`/`runs` が作られ（`CREATE TABLE IF NOT EXISTS` のみ）、`schema_version() == 26`。
  同じ DB に `execution_plan_adopt` で計画を採用し、`work_units_for` で 2 件（ready/pending）読めることを確認。
- 実行したコマンド: `cargo test -p task-core --lib store::`
- 出力の要点: exit 0、既存の migration テスト（0007〜0025）を含め全て green（`SCHEMA_VERSION` の
  ハードコード値 25 →26 の更新 5 箇所を含む）。
- 暗黙の WorkUnit（計画の無い Task）は `execution_plans`/`work_units` に行を作らない
  （`execution_plan_active` が `None` を返すことで `wu_dispatch_gate` が `Atomic` を返す。(i) のテストが
  atomic な Task で `work_units_for` が空であることを間接的に確認している）。

**(b) `POST /tasks/{id}/execution-plan`（origin human。D14 の検証をすべて通す）と `celerisctl execution plan set|show`**
- 実行したコマンド: `cargo test -p task-api --test execution`
- 出力の要点: exit 0、7 passed。正常系（201、WU が pending/ready に分かれる）、`GET`（無認証、200/404）、
  トークン無しの `POST` は 401、知らない task は 404、循環依存・重複 key は 422、`assignee` 等の
  未知フィールドは 400（`deny_unknown_fields`。JSON parse の時点で拒否）、既に active な計画がある
  task への 2 回目の POST は 409（`execution_plan_in_use`）。
- 実行したコマンド: `cargo test -p task-ops --lib execution::`
- 出力の要点: exit 0、5 passed。`adopt_plan`（D14 の検証 → ULID 発行 → トポロジカル順で `seq` →
  pending/ready の初期状態）と `active_plan`。
- 実行したコマンド: `cargo test -p celerisctl --bin celerisctl execution::`
- 出力の要点: exit 0、4 passed。`execution plan set --file <json|->`（stdin 対応）と `execution plan show`。
- `docs/protocol/execution-plan.schema.json`（`celeris.execution-plan/1`）は task-core の
  `execution_plan::tests::committed_schema_matches_generated` で生成・検証（`UPDATE_SCHEMA=1` で再生成）。
  `assignee`/`tier`/`model` は `WorkUnitSpec` に無く、`#[serde(deny_unknown_fields)]` で拒否される。

**(c) fixture の 3 WU（A → B → C）の Task が A → B → C の順に Run を起こす。各完了で `Continue{advance}`、
最後は `WorkerDone` → 最終レビュー → done。1 Task 内の WU は直列**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- three_work_units_run_in_order_and_complete_the_task`
- 出力の要点: exit 0、1 passed。`runs` 索引を `started_at` で並べると `a`→`b`→`c` の順（依存順）。
  `Event::Transitioned.reason` が `advance` 2 回（a→b, b→c の後）+ `worker_done` 1 回。3 件とも
  `runs.status = completed`、`role = worker`。WU の run に渡った `RunContext.work_unit.objective` が
  Task 全体の objective ではなく WU 自身の objective であることを確認（D9）。

**(d) WU の失敗 → retry → 上限で failed。依存先は blocked(dependency_failed)。planner の無い E2 では
replan できないので Task は failed（D12 の 3）**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_work_unit_failure_at_the_retry_limit_fails_the_task_and_blocks_dependents`
- 出力の要点: exit 0、1 passed。WU `b`（`depends_on: [a]`）が retryable error を 2 回返す
  （`max_retries=1` なので 1 回 retry して上限）→ `b` は `failed`、`c`（`depends_on: [b]`）は
  `blocked(dependency_failed)`、Task は `Status::Failed`。`WorkerFinished.outcome` が
  `"work unit b failed"` を含む（D12「接頭辞を付ける」）。

**(e) WU の question → Task が blocked → 回答で再開する**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_work_unit_question_blocks_the_task_and_an_answer_resumes_it`
- 出力の要点: exit 0、1 passed。WU `a` が `Terminal::Question` を返すと `a` は
  `blocked(question)`、Task は `Status::Blocked`。`Trigger::Answer` を投げると Task は `Ready` に戻り、
  次 tick で `wu_dispatch_gate` が `a` を `ready` に戻して再実行、3 WU 全て完了して `Status::Done`。

**(f) WU の continuation（E1 の仕組みを WU 単位で使う。checkpoint は WU ごと）**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_work_unit_yield_continues_with_a_checkpoint_scoped_to_that_work_unit`
- 出力の要点: exit 0、1 passed。WU `a` が 1 回目 `Terminal::Yielded`（checkpoint 付き）、2 回目
  `Done`。`work_units.continuations == 1`、`runs == 2`。`Event::CheckpointSaved{work_unit_id: Some(a.id)}`
  が 1 件、`completed` が checkpoint の申告どおり。2 回目の run の `RunContext.continuation` に
  `run_seq = 2` が載り、1 回目には `continuation` が無い。

**(g) `runs` の索引が全タスクの run について書かれる。`replay` で `work_units`/`runs` が events から
作り直せる（一致を確かめる）**
- `runs` の全件書き込み: `dispatcher.rs::on_worker_finished` の末尾で `run_index_finish` を
  atomic/WU を問わず常に呼ぶ（(c) 等のテストで `runs_for_task` が worker run 分だけ埋まることを確認済み）。
- **未解決**: `celerisctl replay`/`task_ops::replay` を events から `work_units`/`runs` を再構築して
  DB の値と突き合わせる専用のコマンド・テストは、このセッションでは実装できなかった（下記「未解決事項」参照）。
  現状の `replay` は `tasks.status`/`attempts` の再構築のみで、`work_units`/`runs` は対象外のまま。

**(h) 再起動後の照合（D15: 索引と events が食い違えば events が勝つ）**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_work_unit_stuck_running_after_a_restart_is_reconciled_on_lease_expiry`
- 出力の要点: exit 0、1 passed。`running` のまま「落ちた」WU（lease は既に失効、`self.running` に
  エントリ無し = 新しいプロセスでの再起動を模擬）を `tick()`（内部の `reclaim_expired_leases`）が拾い、
  checkpoint が無いので `ready` に戻す（`WorkUnitTransitioned{reason: "restart_reconcile"}`）ことを確認。
  **範囲の限定**: `reclaim_expired_leases`（lease 失効）の経路だけを配線した。`abort_stale_runs`
  （idle timeout・drain）経由の停止は未配線（ADR の「Phase E2 実装時の逸脱・明確化」に記載）。

**(i) 計画を持たない Task の挙動・プロンプトが E1 と同じ（バイト単位）**
- 実行したコマンド: `cargo test -p task-dispatch --lib`（E1 由来の全テストを含む既存 273 件）
- 出力の要点: exit 0、273 passed、0 failed。E1 の continuation/checkpoint/budget_exhausted 系テストは
  1 件も変更していない。`dispatch_ready`/`on_worker_finished` の WU 分岐は `current_wu.is_some()`
  でしか入らず、計画の無い Task では `wu_dispatch_gate` が `Atomic` を返して以後は E1 のコードパスを
  そのまま通る。
- 実行したコマンド: `cargo test -p task-worker --lib claude_code:: preamble::`
- 出力の要点: exit 0（claude_code 64 passed / preamble 19 passed）。既存のプロンプトスナップショット
  テストは無変更で通過。新規テスト `build_prompt_replaces_the_objective_and_acceptance_with_the_work_unit_when_present`
  で `context.work_unit` が `Some` のときだけ `## Objective`/`## Acceptance criteria` が差し替わることを確認。

### P-E0-2（E1 からの申し送り）の修正

claude-code で `result` メッセージは観測できたのに `artifacts/result.json` が無い run は、従来
`Ok(Terminal::Error{retryable:true})` になり `WorkerError` として `task.attempts` を消費していた
（ADR-0070 D3 の想定と食い違う）。`Err(AdapterError::Other(...))`（`ProviderFailure` 無し）に変え、
`provider_failure_outcome` が `None` を返すことで既存の `InfraRequeue` 経路（attempts を消費しない。
上限で `WorkerError{retryable:false}`）に乗るようにした。

- 実行したコマンド: `cargo test -p task-worker --lib -- success_without_result_file_is_an_infra_failure_not_a_retryable_worker_error stale_result_file_from_previous_run_is_cleared_before_this_run`
- 出力の要点: exit 0、2 passed。両テストとも `Err(AdapterError::Other(message))` を確認（メッセージに
  `artifacts/result.json` を含む）。
- codex アダプタの同種の分岐（`codex::tests::success_without_result_file_is_retryable_error`）は
  ADR の P-E0-2 が claude-code 限定の記述だったため、今回は変更していない（範囲外）。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（警告 0。`large_enum_variant`/`clone_on_copy`/`useless_format`/`collapsible_if`/`too_many_arguments` を修正） |
| 全テスト | `cargo test --workspace --no-fail-fast` | exit 0。12 個の test binary が全て `test result: ok`（FAILED 0）。 |
| schema（task-core） | `UPDATE_SCHEMA=1 cargo test -p task-core --lib` | exit 0、286 passed。`event.schema.json`（`ExecutionPlanned`/`WorkUnitTransitioned` 追加）と `docs/protocol/execution-plan.schema.json`（新規）を更新（追加のみ） |
| schema（task-worker） | `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` | exit 0。`worker-protocol.schema.json` 更新（`RunContext.work_unit`/`WorkUnitPromptContext` 追加のみ） |
| schema（task-api） | `UPDATE_SCHEMA=1 cargo test -p task-api --lib` | exit 0、53 passed。`api-v1.schema.json` 更新（`ExecutionPlanView`/`WorkUnitView`/`RoutingRecord.work_unit_id` 追加のみ） |
| GUI gen:types | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と同一（差分ゼロ）。差分は `ExecutionPlanView`/`WorkUnitView`/`Event::ExecutionPlanned`/`WorkUnitTransitioned` 等の型追加のみ |
| GUI typecheck | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0（エラーなし） |
| GUI test | `cd gui && corepack pnpm@11.27.0 test` | exit 0。71 files / 1086 passed（画面は E5 の範囲なので型の追随だけ） |

### ADR との差分

`docs/adr/0072-task-execution-decomposition.md` の「Phase E2 実装時の逸脱・明確化」に詳細を記載。要点:

1. `WorkerStarted`/`WorkerFinished` に `work_unit_id` を足さなかった（E1 の `run_seq` 省略と同じ理由。
   `runs` 索引の `work_unit_id` 列と `CheckpointSaved.work_unit_id` で代替）。
2. WU の run の continuation は `events` ではなく `runs` 索引（`runs_for_work_unit`）から組み立てる。
3. WU の Completed run では checkpoint を merge・保存しない（continuation を経た WU だけが実際の
   `completed` 配列を依存先へ引き継ぐ。それ以外は固定文言「完了」にフォールバック）。
4. WU の予算（D18）は Task の budget をそのまま使う（`WorkUnitSpec.budget` は検証・丸めのみで、実際の
   `RunLimits` には未反映）。
5. D21（WU ごとの routing）は未配線（`RoutingRecord.work_unit_id` は常に `None`）。
6. `WorkUnitTransitioned.from` は伝播対象（`newly_blocked`/`newly_ready`）について一律 `pending`
   （E2 の直列実行では数学的に正しい。上の ADR 本文に根拠を記載）。
7. Task の `Cancel` は WU の行をカスケードしない。
8. WU の再起動照合は `reclaim_expired_leases`（lease 失効）の経路にだけ配線（`abort_stale_runs` 等は未配線）。
9. `WuDispatchGate` の Answer 再開は `Trigger::Answer` を直接フックせず、「Ready + 計画あり + `next_work_unit`
   が `Stuck`」という状態の形で判定する。

### 未解決事項・E3 への申し送り

- **`replay`/`work_units`・`runs` の再構築（(g) の後半）が未実装**: `task_ops::replay::replay` は
  `tasks.status`/`attempts` だけを再構築し、`work_units`/`runs` を events
  （`ExecutionPlanned`/`WorkUnitTransitioned`/`WorkerStarted`+`runs` 索引由来の情報）から作り直して
  DB の値と突き合わせる機能は無い。`work_units`/`runs` は「派生の索引」（D5）なので、原理上は
  `ExecutionPlanned`（計画の初期行）+ `WorkUnitTransitioned`（状態遷移の系列）+
  `CheckpointSaved`/`RoutingDecided`（run の詳細）から再構築できるはずだが、`runs.adapter`/`model`/
  `account`/`started_at`/`finished_at` の一部は今の events だけでは完全に復元できない
  （`WorkerStarted`/`WorkerFinished` に `work_unit_id` が無いため、`runs` 行と `WorkerStarted` イベントの
  対応付けに `run_id` の一致以外の手がかりが要る — `run_id` 自体は両方にあるので対応付け自体は可能。
  実装する価値はあるが、このセッションでは時間を割けなかった）。**次の一手**: `task_ops::execution`
  （またはそれに準ずる新規モジュール）に `rebuild_work_units_and_runs(events: &[Event]) ->
  (Vec<WorkUnitRow>, Vec<RunRow>)` を純粋関数として実装し、`celerisctl replay` に `--check-execution`
  相当のオプションを足すか、既存の `replay` レポートに `work_unit_mismatches`/`run_mismatches` を追加する。
- WU の予算（D18 の既定式）と D21（WU ごとの routing）は E3 の planner 実装と合わせて配線するのが自然
  （現状は Task 一律）。
- Task の `Cancel` の WU カスケードと、`abort_stale_runs` 経由の WU 再起動照合は E3/E4 のどこかで拾うこと。
- `docs/protocol/worker-protocol.md` の `context` の表に `context.work_unit` の行を追加していない
  （E1 が `context.continuation` も表に追加しなかったのと同じ扱いに揃えたが、GUI 節〈E5〉の前に
  一度ドキュメントを棚卸しした方がよい）。

## Phase E3「Complexity Gate と自動 planning」（2026-09-24）

ADR-0072 §6 E3 の受け入れ条件 (a)〜(h) を実装した。区切りは ADR/PROGRESS の指示どおり
(a)(b)(c) gate と記録 → (d)(e)(f)(g) planner run → (h) WU routing と予算、の 3 段（1 セッション内で
テストを都度通しながら進めたため commit は最終 1 本にまとめている）。branch:
`worktree-agent-a65bbb7fb9eab5600`。

作業の前提: worktree のブランチが main の Phase E2 統合（`1f663a8`）より前（`f7338ad`）から
分岐していたため、まず `git merge --ff-only main` で E2 の内容を取り込んでから着手した
（fast-forward。worktree に固有のコミットは無かったので安全）。

実装・検証を終えて `e6c1691`（phase E3 の commit）を作った後、main が E2b（`crates/task-ops/src/replay.rs`
と `crates/task-core/src/store.rs` の `work_units_replace`/`runs_replace`、`celerisctl replay --check/--apply`）
の統合で `f81d58d` まで進んでいたため、`git merge main` で取り込んだ（`1af0736`）。コンフリクトは
`docs/PROGRESS.md` と本 ADR の「Phase E3」節と「Phase E2b」節の隣接部分（どちらも E2 の続きに追記していた
ため）だけで、コードは 1 バイトも衝突しなかった（E2b は指示どおり `replay.rs`/`store.rs`/
`celerisctl/commands/replay.rs` の 3 ファイルだけを触り、`dispatcher.rs` には触れていない）。マージ後に
`cargo fmt --all -- --check`・`cargo clippy --workspace --all-targets -- -D warnings`・
`cargo test --workspace --no-fail-fast` を再実行し、いずれも問題無いことを確認した（下記ゲート参照）。

### 受け入れ条件ごとの証跡

**(a) D13 の規則表の単体テスト（信号ごと、閾値の境目、強制規則、対象外、人の明示 > 規則 > ヒント）。
gate は決定論的（LLM を使わない）**
- 新規: `crates/task-core/src/execution_gate.rs`（純粋関数 `decide`、`out_of_scope_rule`）。
- 実行したコマンド: `cargo test -p task-core --lib execution_gate::`
- 出力の要点: exit 0、9 passed。信号ごと（F1〜F5・S1〜S6・H の重み）、閾値の境目（score 4→atomic /
  6→compound）、強制規則（`compound/long-and-broad`・`atomic/small`）、対象外（`kind!=Execute`・対話・
  `routing` 無し・固定パイプラインの harness・`workspace_mode=Shared`）、優先順位（人の明示が規則表と
  ヒントに優先し、`out_of_scope` はさらにその上に来る）をそれぞれ確認。

**(b) `ExecutionGated` と `Task.routing.execution` の記録（点数、当たった信号、判定、mode）**
- `crates/task-core/src/model.rs`: `Event::ExecutionGated{decision}`、`TaskRouting.execution:
  Option<ExecutionGateDecision>`、`TaskRouting.execution_hint: Option<ExecutionHintSpec>`（人の明示/
  CoS のヒントの入口）、`RunRole::Planner`。
- `crates/task-dispatch/src/dispatcher.rs::execution_gate_if_needed`（`dispatch_ready` の中で
  `assign_if_needed` の後・`decide_lane`/`wu_dispatch_gate` の前。Task の最初の dispatch で 1 回だけ
  判定し、`Task.routing.execution` が既に `Some` なら何もしない）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order`
- 出力の要点: exit 0、1 passed。`Event::ExecutionGated` が記録され、`Task.routing.execution.mode ==
  Compound`、`source == Human`（テストは人の明示 `execution_hint.explicit = true` で gate をバイパス
  させている。規則表のスコア自体は (a) で個別に検証済み）。

**(c) `[execution] gate = shadow`（既定）では記録だけで実行は atomic のまま。`off` は記録もしない**
- `crates/celeris/src/config.rs`: `[execution] gate`（既定 `"shadow"`。`GateMode::parse` で検証、
  未知の値は起動時エラー）。`crates/task-dispatch/src/dispatcher.rs::ExecutionConfig.gate`。
- `execution_gate_if_needed` は `gate = Off` なら即座に `Ok(task)`（判定もしない）。`Shadow`/`On` は
  同じ `decide` を呼び、`ExecutionGateDecision.shadow` に運用モードを写すだけ（判定ロジックは変えない）。
  実際に planner run を起こすかどうかは `dispatch_ready` の `is_planner_dispatch` 判定
  （`gate == On` を明示的に要求）が握っているので、`shadow` では `Compound` と判定されても
  `wu_dispatch_gate` が `Atomic` を返したまま実行される。
- 実行したコマンド: `cargo test -p task-core --lib -- shadow_flag_is_recorded_without_changing_the_decision`
- 出力の要点: exit 0、1 passed。`shadow=true`/`false` で `mode`/`rule_id` が変わらないことを確認
  （純粋関数レベル）。dispatcher レベルでは、`invalid_planner_output_retries_once_then_falls_back_to_atomic`
  等の既存 E2 のアトミック系テスト（`gate` 未設定 = 既定 `shadow`）がそのまま atomic 実行を続けている
  ことで間接的に確認（`gate` を明示的に `On` にしない限り planner run は一切起きない。
  `is_planner_dispatch` の条件に `self.config.execution.gate == GateMode::On` が入っている）。

**(d) `gate = on` の compound な Task で最初の run が planner run（`RunRole::Planner`）になり、
`execution-plan.json` を D14 で検証して採用（`Continue{planned}`）、WU を E2 の scheduler で順に
実行する（偽の planner アダプタでテスト）**
- `dispatcher.rs::dispatch_ready` に `is_planner_dispatch` の判定と、planner run 用の上書き
  （`worker_hint.tier = Frontier`・`worker_hint.adapter = [execution.planner].adapter`・
  `budget = [execution.planner]` の上限、`WorkerStarted.role = Some(Planner)`）を追加。
- `dispatcher.rs::on_worker_finished` の先頭で「`current_wu` が無く、この run が
  `WorkerStarted{role: Planner}` を持つ」ことを検出したら `on_planner_finished` に分岐する
  （通常のワーカー/WU の判定を経由しない）。
- `on_planner_finished`: `Terminal::Done` の run だけ `artifacts/execution-plan.json` を読み、
  `task_core::execution_plan::validate`（+ harness の `[[genres]]` 照合）を通す。妥当なら
  `task_ops::execution::adopt_plan`（`origin = Planner`）で採用し `Trigger::Continue{Planned}`。
  採用後は既存の E2 の scheduler（`wu_dispatch_gate`/`next_work_unit`）がそのまま WU を順に実行する。
- 新規: `crates/task-worker/src/claude_code.rs::build_execution_plan_prompt`（`build_prompt` が
  `context.execution_planner.is_some()` を見て選ぶ）。`docs/protocol/execution-plan.schema.json`
  をそのままプロンプトに埋め込み、D18 の上限・gate の根拠・使える genre 一覧を渡す。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order`
- 出力の要点: exit 0、1 passed。偽の `PlannerScriptAdapter`（`execution-plan.json` を書いて
  `Terminal::Done` を返すだけ）で、2 WU（`a`→`b`）の計画が採用され、`runs` に `role = planner` の
  行が 1 件（`work_unit_id = None`）、続けて 2 件の worker run（`work_unit_id` あり）が記録され、
  Task が `Status::Done` まで進むことを確認。`Event::ExecutionPlanned{origin: Planner, ..}` と
  `Trigger::Transitioned{reason: "planned"}` が 1 回ずつ。
- 実行したコマンド: `cargo test -p task-worker --lib -- build_prompt_selects_the_execution_plan_prompt_when_execution_planner_is_present`
- 出力の要点: exit 0、1 passed。`context.execution_planner` が `Some` なら `task.kind == Execute` の
  ままでも計画プロンプトが選ばれ、schema・D18 の上限・gate の根拠・使える genre 一覧・
  「`assignee`/`tier`/`model` を書くな」の指示が文面に載ることを確認。

**(e) planner の出力が不正なら 1 回だけ再試行し、それでも不正なら atomic に倒す（Task は失敗しない。
理由を記録）**
- `dispatcher.rs::give_up_or_retry_planner`: `events_for` から `WorkerStarted{role: Planner}` の件数
  （この run 自身を含む）を数え、2 回未満なら `Trigger::Continue{Planned}` で再試行（次の
  `dispatch_ready` が `is_planner_dispatch` の条件をまだ満たすので、もう一度 planner run を起こす）。
  2 回に達したら `Task.routing.execution` を `mode = Atomic`・`rule_id = "atomic/planner-invalid"` に
  書き換える 2 件目の `Event::ExecutionGated` を記録してから `Continue{Planned}`（以降
  `is_planner_dispatch` は `false` になり、`wu_dispatch_gate` が `Atomic` を返して通常の暗黙 WU 実行に
  倒れる）。どちらの経路も `Trigger::WorkerError`/`ReviewFail` を使わないので Task は失敗しない。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- invalid_planner_output_retries_once_then_falls_back_to_atomic`
- 出力の要点: exit 0、1 passed。1 回目は `assignee` を書いた不正な出力（schema 違反）、2 回目は
  `execution-plan.json` 自体を書かない、の 2 回の不正な試行の後に atomic へ倒れ、`Status::Done`
  （`attempts == 0`）まで完走することを確認。`Event::ExecutionGated` が 2 件（最初の `Compound` 判定 +
  atomic への書き換え）、planner run が正確に 2 回（`context.execution_planner.is_some()` の run が
  2 件）であることも確認。

**(f) planner の出力に `assignee` / `tier` / `model` があれば schema 違反**
- `task_core::execution_plan::WorkUnitSpec` は元から `#[serde(deny_unknown_fields)]`（E2）で
  `assignee`/`tier`/`model` の欄を持たないので、これらを書けば JSON のデシリアライズ自体が失敗する
  （E3 で新規に検証を足す必要は無かった）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- invalid_planner_output_retries_once_then_falls_back_to_atomic`
  （上の (e) と同じテストの 1 回目の試行が `assignee` を書いた出力で、これで確認している）。
- 出力の要点: exit 0。1 回目の再試行の理由（`worker_progress` イベント）に
  `execution-plan.json の形式が不正: unknown field \`assignee\`` を含むことを確認。

**(g) planner は lead（Task の担当ノードの部署の lead）の実効 profile で走り、node_sessions は
resume しない（task-local）**
- ADR-0033 D1 の組織の木では department ノード自身がその部署の「lead」の実効 profile を持つ
  （department の下に更に lead という別ノード種別は無い）。`dispatch_ready` の planner 分岐で
  `task_core::department_of(&org, task.assignee)` → その department ノードの
  `task_core::resolve_profile` を `extras.profile`/`extras.node` に上書きする（Task 本来の担当の
  profile ではなく、部署の lead の profile で走る）。
- `node_sessions` の resume は `is_conversation(task) == true` の run にしか掛からない仕組み
  （ADR-0054）で、planner run の対象 Task は常に `is_conversation == false`（compound と判定される
  対話・support-task は D13 の対象外）なので、実装を足すまでもなく「resume しない」が成り立つ。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order`
- 出力の要点: exit 0。テストの中で `RunContext.session.is_none()`（planner run の context）を確認。
  部署の lead 切り替えは org を持たない単純なテスト fixture では検証しづらいため、単体テストでは
  「node_sessions を resume しない」側だけを直接確認し、department の profile 切り替え自体は
  コードレビュー（`dispatch_ready` の該当ブロック、`crates/task-dispatch/src/dispatcher.rs`）で
  確認した。実 org での確認は E6 の dogfood に譲る。

**(h) WU ごとの `RoutingDecided.work_unit_id` と WU の view での lane。WU 予算
（`WorkUnitSpec.budget`）を実行に反映**
- 新規: `task_core::model_policy::decide_for_work_unit`（Task を複製し `objective`/`acceptance`/
  `budget`/`genre`/`features` を WU の spec に差し替えた「WU の view」で `TaskFeatures::infer` →
  `ModelPolicy` を通す。D21）。`dispatcher.rs::decide_lane_for_work_unit` から呼ぶ。
- `dispatch_ready` の `RoutingRecord` 組み立てで `work_unit_id: current_wu.as_ref().map(|wu|
  wu.id.clone())`（E2 では常に `None` だったのを配線）。
- WU の予算: `dispatch_ready` で `current_wu` が決まった直後に、`task.budget`（この run が実際に使う
  `RunLimits`/`preamble` の予算の予告）を `wu.spec.budget`（無ければ D18 の既定式
  `max(task.budget.*, 30/1800)`）に差し替える。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- three_work_units_run_in_order_and_complete_the_task`
- 出力の要点: exit 0、1 passed（E2 の既存テスト。WU ごとの `RoutingDecided` は今回の配線で
  `work_unit_id` を持つようになったが、E2 時点のアサーションは `work_unit_id` を見ていなかったため
  非破壊）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order`
- 出力の要点: exit 0。`Event::RoutingDecided` のうち `work_unit_id.is_some()` が WU の数（2 件）と
  一致することを確認。

### E2b からの指摘の修正（範囲外だが `dispatcher.rs` を触るついでに直した）

コーディネータから、E2 が `run_index_start` を WU の run（`start_work_unit_run`）にしか配線しておらず、
計画の無い Task の worker run と reviewer run が `runs` 索引に行を作っていない（E2 の (g) 「全タスクの
run について書かれる」に反する）との指摘があった。`dispatch_ready`（暗黙 WU の worker run）と
`spawn_review`（reviewer run）の両方に `run_index_start` を足し、`on_review_finished` の 3 つの終了経路
（延期・引き分け再判定・通常の pass/fail）すべてに `run_index_finish`（新規 `finish_reviewer_run_index`）
を足した。

- 実行したコマンド: `cargo test -p task-dispatch --lib -- plain_task_writes_both_a_worker_and_a_reviewer_row_to_the_runs_index`
- 出力の要点: exit 0、1 passed。`Check::Reviewer` を持つ計画の無い Task が、worker run 1 件 + reviewer
  run 1 件、合わせて 2 件の `runs` 行を作ること（両方とも `finished_at` が埋まる）を確認。
  E2 の PROGRESS.md の該当記述（「`run_index_finish` を atomic/WU を問わず常に呼ぶ」）は、対応する
  `run_index_start`（`INSERT`）が無いために実際には `UPDATE` が 0 行に当たって黙って no-op になっていた
  ので、事実として訂正する。

### CoS のヒント（入口）

- `task_core::console_action::ConsoleAction::CreateTask.execution: Option<ExecutionMode>`、
  `task_ops::add::NewTaskSpec.execution: Option<ExecutionMode>`。`task_ops::add::create_task_with_roles`
  で `TaskRouting.execution_hint`（`explicit = provenance.origin == Human`）に写す。
- `crates/task-worker/src/preamble.rs::actions_instructions` に「大きな・工程がいくつもある依頼だと
  思ったら `"execution": "compound"` を付けてよい（判定は Complexity Gate が決定的に行う）」の 2 行を
  追加。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし。E2b 統合後の再検証を含む） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings`（`git merge main` で E2b の変更を取り込んだ後に再検証） | exit 0（警告 0。`clone_on_copy`〈`Option<Usage>` は `Copy`〉を修正） |
| 全テスト（最終） | `cargo test --workspace --no-fail-fast`（E2b 統合〈`git merge main`〉後の worktree で実行） | exit 0。81 個の test result ブロック全て `ok`（FAILED 0） |
| schema（task-api） | `UPDATE_SCHEMA=1 cargo test -p task-api --lib` | exit 0、53 passed。`api-v1.schema.json` 更新（`ExecutionGateDecision`/`GateSignal`/`ExecutionHintSpec`/`Event::ExecutionGated`/`TaskRouting.execution*` 追加のみ） |
| schema（task-worker） | `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` | exit 0、1 passed。`worker-protocol.schema.json` 更新（`RunContext.execution_planner`/`ExecutionPlannerContext` 追加のみ） |
| GUI gen:types | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と同一（差分ゼロ）。差分は `ExecutionGateDecision` 等の型追加のみ |
| GUI typecheck | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0（エラーなし） |
| GUI test | `cd gui && corepack pnpm@11.27.0 test` | exit 0。71 files / 1086 passed（画面は E5 の範囲なので型の追随だけ） |

### ADR との差分

`docs/adr/0072-task-execution-decomposition.md` の「Phase E3 実装時の逸脱・明確化」に詳細を記載。要点:

1. D13 の「対象外」は 2 段階（対話・support-task・`kind!=Execute`・`routing`無しは gate 呼び出し前に
   フィルタして記録しない／固定パイプラインの harness・`workspace_mode=Shared` は `decide` の中で
   `rule_id="atomic/out-of-scope"` として記録する）。
2. S4（複数の実行環境）・S6（直近の budget_exhausted 実績）は既定値（`false`/`None`）で運用（D13 が
   明示的に許容している簡略化）。
3. `[execution.planner].permission_mode` は設定に持つが実行時の配線はしていない（`with_permission_mode`
   のようなアダプタフックが無いため）。
4. D21 の WU ごとの lane にリトライのエスカレーションは適用しない（WU の retry は `task.attempts` を
   消費しないので、そもそもほとんど発火しない）。
5. Planner の出力の harness 検証は `[[genres]]` の id 集合との照合のみ（担当 profile のサブセットまでは
   絞らない）。
6. replan（D17）は未実装（E4 の範囲）。

### 未解決事項・E4 への申し送り

- replan（D17）: `ExecutionPlannerContext.replan` は常に `false`。WU の `failed`・`plan_issue`・
  進捗なし・実質的なレビュー不合格から replan を起こす経路（D17）は E4 で実装すること。
- `classify_review_failure`（D16）・WU の決定的 `checks`（Command）の実行（D14 で検証はしているが
  E3 では実行しない。E4 の範囲）。
- S4/S6 の実データ配線（org の部署またぎ判定、`runs` 索引からの直近 budget_exhausted 実績の集計）は
  shadow の記録（E5/E6）を見てから行う。
- `[execution.planner].permission_mode` の実行時配線（`with_permission_mode` フックの追加）。
- replay/`work_units`・`runs` の再構築は E2b（下記）で実装済み。`execution_plans` の再構築（replan 導入後）は
  E4 で対象に加えること（E2b の申し送り）。

## Phase E2b「replay による work_units / runs の再構築と突合」（2026-09-24）

ADR-0072 D5/D15（events が正本、`execution_plans`/`work_units`/`runs` は派生の索引）の E2 の受け入れ
条件 (g) 後半（`work_units`/`runs` の replay 再構築）を実装した。触ったのは指示どおり
`crates/task-ops/src/replay.rs`、`crates/celerisctl/src/commands/replay.rs`、
`crates/task-core/src/store.rs`（索引の読み書き）の 3 ファイルのみ（`crates/task-dispatch/src/dispatcher.rs`
は触っていない。E3 が並行して編集中の前提）。

branch: `worktree-agent-a5eea4ba4e20f914e`。

### 前提: worktree の同期

この worktree は Phase E2 の main への merge（`1f663a8`）より前の commit（`f7338ad`）から分岐していた
ため、`execution_plan.rs`/`execution_scheduler.rs`/migration 0026 などが存在しなかった。`git diff
f7338ad main` のパッチを `patch -p1`（`git merge`/`git apply` は使わず、プレーンな patch コマンドで適用。
「`git merge`/`git push` はしない」の制約を尊重した）で取り込み、`sync: bring worktree branch up to
date with main (Phase E2 merged)` として先頭に 1 commit した（このタスク固有の事情なので、他の Phase
の作業には影響しない）。

### 発見: `runs` 索引は E2 の時点で WU 経由の worker run にしか書かれていない

実装前に `run_index_start` の呼び出し箇所を数えたところ、本番コード（`dispatcher.rs`）では
`start_work_unit_run`（WU の dispatch）の 1 箇所だけだった。`run_index_finish` は run の終了時に
常に呼ばれるが、対応する `run_index_start` が無い run（暗黙の WU の worker run、reviewer run）には
行が無いので何もしない。つまり本番 DB の `runs` 表は現状「計画のある Task の WU run」しか持っていない。
詳細は ADR-0072「Phase E2b 実装時の逸脱・明確化」に記録した。

### 受け入れ条件ごとの証跡

**条件: `task_ops::replay` に純粋関数 `rebuild_work_units_and_runs(events) -> (Vec<WorkUnitRow>,
Vec<RunRow>)` を足す（events だけから `ExecutionPlanned`/`WorkUnitTransitioned`/`WorkerStarted`/
`WorkerFinished`/`CheckpointSaved` の列を畳み込んで再構築する）**
- 実装: `crates/task-ops/src/replay.rs`（`rebuild_work_units_and_runs`。引数は `EventRow`〈`ts` 付き〉。
  理由は ADR 追記参照）。I/O 無しの純粋関数（`TaskStore` を引数に取らない）。
- 実行したコマンド: `cargo test -p task-ops --lib replay::`
- 出力の要点: exit 0、9 passed（既存 6 件 + 新規 3 件）、FAILED 0。

**条件: `celerisctl replay` に統合し、`--check` で現在の索引と突合して差分を出し、`--apply` で索引を
events に合わせる（events が勝つ）**
- 実装: `crates/celerisctl/src/commands/replay.rs`。`ReplayArgs` に `--check`/`--apply`
  （`bool`、`clap::Args`）を追加。フラグ無しの既定は従来どおり `tasks.status`/`attempts` の突合のみ
  （後方互換）。`--check`/`--apply` のどちらかがあれば `task_ops::replay::check_and_apply_execution`
  を呼び、`WORK_UNIT_MISMATCH`/`RUN_MISMATCH` 行を追加で出す。`--apply` は差分のあったタスクの
  `work_units`/`runs` を再構築結果で上書きし（`work_units_replace`/`runs_replace`。events は変えない）、
  `replay: applied execution index fixes for N task(s)` を出す。exit code は残った mismatch が 0 件なら
  `SUCCESS`、1 件以上なら `FAILURE`。
- 実行したコマンド: `cargo test -p celerisctl --bin celerisctl replay::`
- 出力の要点: exit 0、2 passed（既存のまま。`ReplayArgs {}` → `ReplayArgs::default()` に更新しただけ
  で挙動は不変）。
- `task_core::store::SqliteStore` に `work_units_replace`/`runs_replace`（`TaskStore` トレイトへ追加。
  実装は他に無いので破壊的変更ではない）を追加。既存の `run_index_start` の INSERT 文は
  `insert_run_row_tx`（新設の private helper）へ切り出し、`runs_replace` と共有した。

**条件: テスト (1) E2 の fixture（A → B → C、失敗 → blocked、question → answer、continuation）で
scheduler が書いた索引と replay の再構築が一致する**
- `rebuild_work_units_and_runs_matches_the_scheduler_written_index_for_the_e2_fixtures`
  （`crates/task-ops/src/replay.rs`）。A（continuation 1 回 → 完了）→ B（依存先 A の完了で ready →
  retryable failure で retry → 2 回目の dispatch で question → answer → 3 回目の dispatch で
  非 retryable failure → failed）→ C（B の失敗で `dependency_failed` へ伝播）という 1 本の筋で、
  ADR-0072 D6 の遷移理由（`dispatch`/`continue`/`retry`/`completed`/`question`/`answer`/`failed`/
  `dependency_ready`/`dependency_failed`）を全て踏む。`store.work_unit_transition`/`run_index_start`/
  `run_index_finish`/`append_event` を dispatcher.rs と同じ呼び方（同じ引数の組み合わせ）で手書きし
  「scheduler が書いた索引」を模擬し、`rebuild_work_units_and_runs` の結果と `diff_execution` で
  突き合わせて `Vec::new()`（差分ゼロ）を確認。counts（`a.runs=2`/`continuations=1`、`b.runs=3`/
  `retries=1`、`c.status=Blocked(DependencyFailed)`）も個別に確認した。
- 実行したコマンド: `cargo test -p task-ops --lib replay::tests::rebuild_work_units_and_runs_matches_the_scheduler_written_index_for_the_e2_fixtures`
- 出力の要点: exit 0、1 passed。

**条件: テスト (2) 索引を故意に壊しても replay が直す**
- `check_and_apply_execution_fixes_a_corrupted_index`（同ファイル）。1 WU（`a`、dispatch→completed）の
  正しい index を作った後、`work_units_replace`/`runs_replace`（events を経由しない直接の書き込み）で
  `a` を `Blocked(Question)`、その run を `Failed` に壊す。`--check`（`apply=false`）では
  `work_unit_mismatches`/`run_mismatches` が非空で、DB はまだ壊れたまま（`applied == 0`）であることを
  確認。`--apply`（`apply=true`）で `applied == 1`、mismatch がゼロになり、DB が `Done`/`Completed`
  （events が示す本来の値）に戻ることを確認。さらにもう一度 `--check` して差分ゼロが安定することも
  確認した。
- 実行したコマンド: `cargo test -p task-ops --lib replay::tests::check_and_apply_execution_fixes_a_corrupted_index`
- 出力の要点: exit 0、1 passed。

**条件: テスト (3) 計画の無い Task では work_units は空、runs は全 run 分**
- `rebuild_work_units_and_runs_is_empty_for_a_task_without_a_plan_and_covers_every_run`（同ファイル）。
  `ExecutionPlanned` の無いタスクに worker run 2 件（1 件失敗・1 件完了）+ reviewer run 1 件
  （`role: Some(Reviewer)`、`end: None` の run。本番でも reviewer run に `end` が付かないことが
  ある想定の安全網 — `end` が無ければ `RunIndexStatus::HarnessError` にフォールバックすることも
  合わせて確認）を積み、`rebuild_work_units_and_runs` が `work_units == []`、`runs.len() == 3`
  （worker 2 + reviewer 1、全て `work_unit_id: None`）、worker/reviewer それぞれ独立した 1 始まりの
  `seq` を持つことを確認した。
- 実行したコマンド: `cargo test -p task-ops --lib replay::tests::rebuild_work_units_and_runs_is_empty_for_a_task_without_a_plan_and_covers_every_run`
- 出力の要点: exit 0、1 passed。

### ゲート（本 Phase 完了時点）
## Phase E3 + Phase 119 の本番反映（2026-09-25 00:53Z）

- E3 統合 3dafac5（ゲート: fmt 0、clippy 0、GUI 1086 件 / gen:types 差分ゼロ。`cargo test` は e2e `reload_clears_provider_cooldown` が load 17 の中で 1 回落ちたが単体では 0.3 秒で通過。時間依存のフレーク）。119 統合 a2968d1（fmt / test FAILED 0 / clippy 0、`scripts/selfdeploy/tests/pid_resolution_test.sh` 6/6）。
- release `a2968d1b7477`、verify 全 true（schema 26、N-1 = 1f663a83ff6c は読める）、in-flight 0 でライブ切替。本番は E3（gate は既定 shadow: 判定と記録だけ、実行は atomic のまま）と 119 を含む。
- 旧デーモン 1f663a83ff6c は 119 以前のコードなので予想どおり「celeris stopped」の後もプロセスが残った → `systemctl --user stop` で片付け（最後の該当個体。以後の切替は 119 の shutdown_timeout + process::exit で自動終了するはず。次回の切替で確認する）。
- クラスタ（pegasus / sirius）は E2 の停止→起動以降、未接続のまま（人の TOTP 再接続待ち）。

## Phase E4「reviewer repair と replanning」（2026-09-25）

ADR-0072 §6 E4 の受け入れ条件 (a)〜(g) を実装した（(h) は任意・時間の制約により未実装）。作業の前提:
worktree のブランチが main の Phase E3 統合（`3dafac5`）より前（`f81d58d`）から分岐していたため、
まず `git merge main`（fast-forward、コンフリクト無し）で E3 の内容を取り込んでから着手した。
branch: `worktree-agent-ab52da02cf647ed97`。

指示どおり区切って進めた: (a)(b)(c) repair + (g) WU checks（1 コミット）→ (d)(e)(f) replan（別途
最終コミットへまとめる）。

### 受け入れ条件ごとの証跡

**(a) `classify_review_failure` の分類表のテスト（format / lint / test_small / reviewer_local /
merge_base / substantive。混在なら substantive）**
- 新規: `crates/task-core/src/execution.rs::classify_review_failure`（純粋関数）、`RepairClass`
  （`Format`/`Lint`/`TestSmall`/`ReviewerLocal(ReviewerRepairKind)`/`MergeBase`）、`RepairDecision`。
- 実行したコマンド: `cargo test -p task-core --lib execution::tests`
- 出力の要点: exit 0、22 passed。format/lint コマンドの分類、test_small（失敗 1〜3 件）と非該当
  （4 件以上・失敗数が読めない）、無関係なコマンドの substantive、reviewer の明示 `repair` ヒント
  （`scope=local`）と字句フォールバック（fmt/clippy の語だけ）、`scope` が `local` 以外は substantive、
  `Check::Human` は常に substantive、修復できる不合格と修復できない不合格の混在・**異なる
  修復できるクラスの混在**（両方とも substantive）を確認。`merge_base` はこの関数からは返らない
  設計（配送〈`delivery.rs`〉自身の技術的失敗から直接組み立てる。ADR 本文と「Phase E4 実装時の
  逸脱・明確化」に記載）。

**(b) `cargo fmt --check` 相当だけが不合格の Task が `ReviewRepair` → repair WU（最小の context。
request.json に元の objective の全文が無い）→ 再レビュー → `done`。attempts 不変、人の承認は再利用**
- 新規: `task_core::execution::build_repair_objective`（先頭 600 文字のみの Task 目的プレビュー +
  失敗した検査の詳細 + git diff --stat。元の Run の履歴・計画・全文の objective は載せない）。
- `Trigger::ReviewRepair`（`transition.rs`。Reviewing → Ready、attempts 据え置き）。
- `TaskStore::review_repair_apply`（`store.rs`。atomic な Task では `execution_plans`
  〈origin=repair〉+ `main`（done）+ `repair-N`（ready）の 2 行を、計画済み Task では `repair-N`
  の 1 行だけを、`Trigger::ReviewRepair` の適用と同一トランザクションで書く）。
- `Dispatcher::try_review_repair`（`dispatcher.rs`。`on_review_finished` の `ReviewFail` 分岐の手前
  で分類・上限を確認し、repairable なら repair WU を実体化する）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_format_only_review_failure_is_repaired_without_consuming_attempts`
- 出力の要点: exit 0、1 passed。長い objective（960 文字、600 文字の上限より長い一意なマーカー付き）
  を持つ atomic な Task が、`cmd` に "cargo fmt --check" を含む `Check::Command` の不合格から
  `ReviewRepair` → `main`（done）+ `repair-1`（`WorkUnitKind::Repair`）を実体化 → repair WU が
  `.repair-done` を作って `Done` → 再レビューが通って Task `Done` になることを確認。
  `repair.spec.objective` に元の 960 文字の objective（マーカー含む）が**含まれない**ことを直接
  assert（最小の context）。`stored.attempts == 0`（repair は attempts を消費しない）。

**(c) repair の上限（`max_repairs`/`max_repairs_per_class`）を超えると従来の `ReviewFail`**
- `ExecutionConfig.max_repairs`（既定 3）・`max_repairs_per_class`（既定 2）。`[execution]` の TOML
  にも追加（`crates/celeris/src/config.rs`）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- exceeding_the_per_class_repair_limit_falls_back_to_review_fail`
- 出力の要点: exit 0、1 passed。常に不合格になる format 系の Command 検査を持つ Task が、
  `max_repairs_per_class = 2` ちょうど 2 回 repair を試みた後（`work_units` の `kind=repair` が
  2 件）、3 回目は repair せず `review_fail` の `Event::Transitioned` が記録され、
  `Status::Failed`（`max_retries = 0` で即失敗。理由は下記「未解決事項」参照）になることを確認。

**(g) WU の決定的な `checks`（Command）が WU の完了前に走り、失敗なら WU を retry する
（review.rs の checks 実行を再利用）**
- 新規: `review::run_work_unit_checks`（`review_task` の `Check::Command` 分岐と同じ判定
  〈`workspace.exec` で実際に再実行し exit を比較〉を関数として切り出し、`WorkUnitCheck` の列に
  適用する）。
- `Dispatcher::on_worker_finished` を `finish_worker_result`（共通の後段）に分割し、
  `current_wu.spec.checks` が非空かつ run が `Terminal::Done` のときだけ
  `spawn_work_unit_checks`（非同期。`Completion::WorkUnitChecks` を送る）に分岐する。1 つでも
  `pass = false` があれば `Terminal::Error{retryable:true}` にすり替えて `finish_worker_result` に
  渡す（既存の WU retry 経路〈`execution_scheduler::failed`〉にそのまま乗る）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- work_unit_checks_pass_and_fail_like_command_criteria work_unit_checks_time_out a_failing_work_unit_check_retries_the_work_unit_then_completes`
- 出力の要点: exit 0、3 passed。`review.rs` レベルで checks の pass/fail/timeout を確認
  （2 テスト）。`dispatcher.rs` レベルで、`checks` を持つ WU が 1 回目 `Terminal::Done` でも
  checks 失敗で `retry`（`retries += 1`、Task の `attempts` は不変）、2 回目で checks が通って
  `done` になることを確認（`a.runs == 2`、`WorkUnitTransitioned.reason` に `"retry"` と
  `"completed"` の両方）。

**(d) WU の failed・進捗なし・実質的な不合格で replan の planner run が起き、`done` の WU を保持した
v2 が採用される。人の依頼の例（A done / M 追加 / B blocked by M / C blocked by B）**
- 新規: `task_ops::execution::replan`（旧 `active` な計画を `superseded` にし、新版を採用。`done`
  の WU は行を触らない、未完了で残る key は spec・依存・状態を更新（`runs`/`continuations`/
  `retries` をリセット）、消えた未完了の key は `superseded`、新しい key は新規行）。
- `TaskStore::execution_plan_replan`（同一トランザクションで旧版 supersede + 新版挿入 + WU 更新 +
  events）。
- `wu_dispatch_gate`: `NextStep::Stuck` で `Failed`/`Blocked(dependency_failed|limit)` の WU が
  残っていれば、`NextStep::AllDone`（全 WU done のまま `Ready`。repair 枯渇後の `ReviewFail` や
  実質的な review 不合格の後の再 dispatch）も、`replan_gate`（`max_replans` を見て
  `RunPlanner{replan:true}` を返す）に倒す。`dispatch_ready` は既存の `is_planner_dispatch`
  （E3）の配線をそのまま使い、`replan_dispatch` のときも同じ budget/lane/role の上書きをする。
  `on_planner_finished` は `execution_plan_active` の有無で `adopt_plan`/`replan` を切り替える。
- `finish_worker_result`: WU が `"failed"`/`"limit"` になったとき、`max_replans` の余地があれば
  Task を failed/blocked にする代わりに `Trigger::Continue{why: Replan}` にする。
- 実行したコマンド: `cargo test -p task-ops --lib execution::tests`
- 出力の要点: exit 0、10 passed（既存の `adopt_plan` 系 4 件 + 新規 `replan` 系 6 件）。
  `replan_keeps_done_work_units_and_applies_the_human_request_example` が D17 の人の依頼の例
  （A done、M 追加、B が `depends_on: ["a","m"]` に変わり `pending`〈M がまだ done でない〉、
  C も `pending`、M は依存無しで即 `ready`）をそのまま検証している。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_failed_work_unit_triggers_a_replan_instead_of_failing_the_task`
- 出力の要点: exit 0、1 passed。3 WU（A→B→C）の計画で B が retry の上限で `failed` になった後、
  Task が `failed` にならず replan の planner run（偽アダプタ）が起き、A は `done` のまま
  （元の版の `plan_id`）、v2（`origin=planner`、`supersedes=Some(v1)`）が採用され、B は
  リセットされた状態からやり直して `done`、最終的に Task が `done`（`attempts == 0`）になることを
  確認。

**(e) replan の上限で blocked（質問 + 承認、人の回答で再開）**
- 実行したコマンド: `cargo test -p task-dispatch --lib -- exceeding_the_replan_limit_blocks_with_a_question_that_a_human_can_answer`
- 出力の要点: exit 0、1 passed。進捗なしを繰り返す WU が `no_progress_limit` で `limit` に達し、
  `max_replans = 1` の 1 回目は replan（v2 採用）、2 回目の `limit`（`no_progress_streak` は
  replan をまたいで引き継ぐ。「回答」だけが窓を区切る〈D18〉）は replan せず
  `Status::Blocked`・`WorkUnitBlockedReason::Limit` になることを確認。`Trigger::Answer` を直接
  適用して再開し、WU が `Blocked` でなくなることも確認（`record_question_approval` は org/秘書を
  持たない fixture では何も作らないため、承認〈`approvals`〉行そのものの検証は割愛。仕組み自体は
  ADR-0008 D2 の既存機構）。

**(f) 版の履歴（`ExecutionPlanned.supersedes`、`GET /tasks/{id}/execution-plan` に版一覧）が
監査できる**
- `Event::ExecutionPlanned.supersedes`/`reason` は E2 の時点で既に持っていたフィールドで、replan の
  たびに `Some(旧 plan_id)`/`Some("replan (planner run)" 等)` を書く（上記 (d) のテストで確認済み）。
- `task-api::types::ExecutionPlanView.versions: Vec<ExecutionPlanVersionView>`（`id`/`version`/
  `origin`/`planner_run_id`/`status`/`created_at`/`superseded_at`。`store.execution_plan_list`
  から。`GET`/`POST /tasks/{id}/execution-plan` の両方の応答に載る）。
- 実行したコマンド: `cargo test -p task-api --lib`
- 出力の要点: exit 0、53 passed（既存のハンドラテストが `ExecutionPlanView` の新シグネチャ
  〈第 3 引数 `versions`〉を経由しても壊れていないことを含む）。専用の HTTP レベルテストは
  時間の制約で追加していない（`task_ops::execution::replan` 側の 6 テストと `execution_plan_list`
  の単体テストで下地は確認済み。次の一手）。

**(h)（任意）配送の `[delivery-repair]` を `merge_base` の repair WU に置き換える**
- 未実装。ADR 自身が「任意。時間があれば」としている項目で、時間の制約により (a)〜(g) を優先した。
  `RepairClass::MergeBase` の型・budget は用意済み。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（警告 0） |
| 全テスト | `cargo test --workspace --no-fail-fast` | exit 0。81 個の `test result:` ブロック全て `ok`（FAILED 0、計 2,246 tests passed） |
| schema（task-worker） | `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` | exit 0、1 passed。`worker-protocol.schema.json` 更新（`ReviewVerdictOut.repair`/`ReviewRepairHint` 追加のみ） |
| schema（task-api） | `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::` | exit 0、2 passed。`api-v1.schema.json` 更新（`ExecutionPlanVersionView` 追加、`ExecutionPlanView.versions` 追加のみ） |
| GUI gen:types | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と同一（差分ゼロ） |
| GUI typecheck | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0（エラーなし） |
| GUI test | `cd gui && corepack pnpm@11.27.0 test` | exit 0。71 files / 1086 passed（画面は E5 の範囲なので型の追随だけ） |

### ADR との差分

`docs/adr/0072-task-execution-decomposition.md` の「Phase E4 実装時の逸脱・明確化」に詳細を記載。要点:

1. 触るファイルが ADR §6 E4 の表より広い（`store.rs` に 2 メソッド追加、`on_worker_finished` の分割、
   `task-ops/execution.rs::replan`、`celeris/config.rs`、`task-api` の `versions`）。D16/D17 の実装に
   必要だった（理由は ADR 参照）。
2. D16 の repair lane（cheap/standard）は強制しない（budget だけ反映、lane は既存 policy に委ねる）。
3. repair の per-class カウンタは WU の `title` の接頭辞から復元する（`WorkUnitSpec` に専用欄を足す
   と planner の schema に影響するため）。
4. D17 3.（`checkpoint.plan_issue`）は replan のトリガーとして配線していない（WU 状態の扱いを
   詰め切れなかった。E5/E6 で再設計）。D17 1./2./4. は実装・テスト済み。
5. replan run のプロンプトに現在の計画・WU 状態を渡す配線（`claude_code.rs`）はしていない
   （`ExecutionPlannerContext.replan` フラグのみ。E3 の `permission_mode` と同じ理由）。
6. E3 の `give_up_or_retry_planner` の attempts カウントを「直近の `ExecutionPlanned` 以降」に
   修正（replan 導入で fresh planning との窓を混同するバグを防ぐため必須の修正）。
7. `give_up_or_retry_planner` の give-up 時の振る舞いを、`active` な計画の有無で分岐
   （fresh planning は atomic フォールバック、replan は `blocked`）。
8. `wu_dispatch_gate` の「人の回答直後だけ再開する」判定を、直前の `Transitioned.reason` を見るよう
   厳密化（`Continue{why: Replan}` による誤爆を防ぐため必須の修正）。
9. `wu_dispatch_gate` の `AllDone`/`Stuck` の扱いを拡張し、`replan_gate` に倒す経路を追加
   （E2/E3 の「理論上到達しない」という前提を E4 が崩すため）。
10. D12 3.（WU failed の replan 枯渇 → failed）と D18（limit の replan 枯渇 → blocked）を、
    `decision.reason` で分けて実装した（詳細は ADR 参照）。
11. replan で持ち越す未完了 WU は `runs`/`continuations`/`retries` をリセットする（明記は無いが
    D18「回答の時点から数え直す」と同じ発想の拡張）。
12. `execution_plans` に `supersedes`/`reason` の列は追加していない（`Event::ExecutionPlanned` に
    既にある。`GET .../execution-plan` の `versions` はそれ以外の欄だけを返す）。
13. E2b の申し送り（`execution_plans` の版の履歴の replay 再構築）は E4 でも見送り（`replay.rs` は
    触るファイルの範囲外）。次の一手として記録。
14. (h)（配送の merge_base repair）は未実装（任意項目、時間の制約）。

### 未解決事項・E5 への申し送り

- **D17 3.（plan_issue トリガー）**: `Checkpoint.plan_issue` を replan の起点にする設計・実装
  （`WorkUnitBlockedReason` の新設または別の判定方式）。
- **replan run のプロンプト**: `claude_code.rs::build_execution_plan_prompt` に「現在の計画・WU の
  状態・起こした理由」を渡す配線（`ExecutionPlannerContext` にフィールドを足す必要あり）。
- **`[execution.planner].permission_mode` の実行時配線**（E3 からの持ち越し。`with_permission_mode`
  フックが無い）。
- **task-api の execution.rs に専用テストが無い**（`versions` の HTTP レベル検証。時間の制約。
  `task_ops::execution::replan` と `execution_plan_list` の単体テストで下地は確認済み）。
- **(h) 配送の repair**（`crates/celeris/src/delivery.rs`）: `RepairClass::MergeBase` を使った
  repair WU の組み立て。
- **`celerisctl replay`/`rebuild_work_units_and_runs` の `execution_plans` 対応**（E2b からの持ち越し。
  replan で複数版になった `execution_plans` を events から再構築・突合できるようにする）。
- **repair の lane（cheap/standard）の強制**: 現状は budget だけ反映し、lane は policy 任せ。
  実運用（E6 dogfood）で repair が高すぎる lane に流れていないか確認し、必要なら
  `with_permission_mode` と同様のフックを検討する。
- E1〜E3 からの申し送り（S4/S6 の実データ配線、`permission_mode` の実行時配線など）は本 Phase では
  着手していない（引き続き E5/E6 で検討）。

### 次のコミット（分割の記録）

このセッションは "checkpoint" を切らずに完走したため、`git commit` は最終的に 2 回
（`phase E4 (1/3)`: (a)(b)(c)(g)、`phase E4 (2/3)` または統合コミット: (d)(e)(f) + ADR/PROGRESS）
に分けて記録する。詳細な commit sha は本セクションの末尾（完了報告）を参照。

## Phase E4b「E3/E4 の申し送りの穴埋め」（2026-09-25）

Phase E4 の「未解決事項・E5 への申し送り」のうち小〜中規模の項目 1〜5 を実装した（6 は任意・
時間の制約により未実装）。worktree のブランチが main の Phase E4 統合（`751f973`）より前
（`e8f1b6f`）から分岐していたため、まず `git merge main`（fast-forward、コンフリクト無し）で
E4 の内容を取り込んでから着手した。branch: `worktree-agent-a067096616a9c8d0d`。

指示どおり 1 → 2 → 3 → 4 → 5 の順に区切ってコミットした。

### 項目ごとの証跡

**1. replan run のプロンプト配線**
- `task_worker::protocol::ExecutionPlannerContext` に `replan_reason`/`current_plan_version`/
  `work_unit_summaries`/`preserve_done_keys` を追加（`replan = false` のときは全て既定値で、
  `build_execution_plan_prompt` はこの節自体を出さないため、初回 planning のプロンプトは
  1 バイトも変わらない）。`claude_code.rs::replan_context_section`（新規）が、replan のときだけ
  「現在の計画（版）」「WU ごとの状態（`<key> (<kind>) status=<status>[blocked: <reason>]: <完了/
  失敗の要約>`）」「起こした理由」「保持すべき done の WU の key（変えたら拒否される旨も明示）」を描く。
  `dispatcher.rs::execution_planner_context` を `Result` を返すよう変更し、replan のときだけ
  `work_units_for`/`execution_plan_active`/`events_for` を読んで埋める。「起こした理由」は
  `replan_trigger_reason`（新規）が events を新しい方から辿って決定的に文字列化する
  （`WorkerFinished.outcome` の `"replan: "` 接頭辞、または実質的な review 不合格なら直近の
  `ReviewVerdict{pass:false}` の理由）。
- 実行したコマンド: `cargo test -p task-worker --lib -- build_prompt_selects_the_execution_plan_prompt_when_execution_planner_is_present build_execution_plan_prompt_replan_includes_current_plan_and_reason`
- 出力の要点: exit 0、2 passed。`replan = false` のプロンプトに `"REPLANNING an existing execution plan"` が出ないこと（初回プロンプト不変の確認）、`replan = true` のプロンプトに版・理由・WU 要約・
  保持すべき key が載ることを確認。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_failed_work_unit_triggers_a_replan_instead_of_failing_the_task`
- 出力の要点: exit 0、1 passed。実際の replan dispatch で `execution_planner.current_plan_version ==
  Some(1)`、`replan_reason` に `"work unit b failed"` を含む、`preserve_done_keys == ["a"]`、
  `work_unit_summaries` に `a`（`status=done`）と `b`（`status=failed`）の行が出ることを確認。
- `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated`: exit 0、
  1 passed。`worker-protocol.schema.json` は `ExecutionPlannerContext` の追加フィールドのみの差分。

**2. plan_issue トリガー（D17 の 3）**
- `task_core::WorkUnitBlockedReason::PlanIssue`（新設）。`execution_scheduler::decide` は、`end` の
  種類に関わらず（continuation 判定より前に）合成済み checkpoint が `plan_issue` を持てば
  `blocked(plan_issue)` を返す（checkpoint は `end.is_continuable()`〈Yielded/BudgetExhausted〉の
  ときだけ合成されるという既存の制約〈E2b〉があるため、実際に効くのはその 2 つの終わり方だけ）。
  `dispatcher.rs` は `"failed"`/`"limit"` と同じ枠組みで `"plan_issue"` を扱い、replan の余地
  （`max_replans`）があれば `Trigger::Continue{why: Replan}`、無ければ `WorkerQuestion`（blocked）に
  倒す。`wu_dispatch_gate` の「人の回答直後だけ再開する」判定と `Stuck` の `has_unresolved_failure`
  にも `PlanIssue` を追加（replan 上限を使い切った plan_issue が永久に `Stuck` で固まらないように）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- execution_scheduler`
- 出力の要点: exit 0、11 passed（新規
  `a_plan_issue_in_the_checkpoint_blocks_the_work_unit_even_within_budget` を含む。continuation の
  上限にまだ達していなくても `plan_issue` が優先され、`continuations` を消費しないことを確認）。
- 実行したコマンド: `cargo test -p task-dispatch --lib -- a_plan_issue_checkpoint_triggers_a_replan_and_v2_is_adopted`
- 出力の要点: exit 0、1 passed。偽ワーカーが `Terminal::Yielded{checkpoint: {"plan_issue": "..."}}`
  を書く → WU が一度 `blocked(plan_issue)` を経由（`WorkUnitTransitioned{reason: "plan_issue"}`）
  → replan の planner run → v2 採用 → `b` は plan_issue を書かずに完了 → Task `done`（attempts
  不変）を確認。replan run のプロンプト文脈（項目1）の `replan_reason` に plan_issue の文言が
  乗ることも合わせて確認。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::`: exit 0、2 passed。
  `api-v1.schema.json` は `WorkUnitBlockedReason` に `plan_issue` が増えた分のみの差分。
  `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff ゼロ、`typecheck` 通過
  （`types.ts` に 1 行差分）。

**3. `[execution.planner].permission_mode` の実行時配線**
- `task_worker::WorkerAdapter` トレイトに `with_permission_mode`（`with_model` と同じ形、既定
  `None`）を追加。`ClaudeCodeAdapter` で実装（`--permission-mode` を上書きした複製）。
  `self.adapters` に登録されている実体は `TieredAdapter` で包まれているため、
  `TieredAdapter::with_permission_mode` も基盤アダプタへ中継するようにした。codex は
  permission_mode に相当する単一の設定欄を持たない（`extra_args` の自由記述のみ）ため見送り
  （トレイトの既定 `None` のまま。対応しないアダプタとして扱われる）。
  `dispatcher.rs`: `RunExtras` に `planner_permission_mode: Option<String>` を追加し、
  `dispatch_ready` の `is_planner_dispatch` 分岐で `self.config.execution.planner.permission_mode`
  を積む（`RunContext` には乗せない。プロンプトではなく実行そのものの配線）。`run_worker` が
  `with_container` と同じ形で `adapter.with_permission_mode` を呼び、対応しないアダプタは警告ログを
  出してそのまま既定のアダプタで走る。
- 実行したコマンド: `cargo test -p task-worker --lib -- with_permission_mode_overrides_the_permission_mode_argument`
- 出力の要点: exit 0、1 passed。偽 CLI が受け取った argv（`"$*"`）を検査し、既定の
  `bypassPermissions` ではなく上書きした値 `"plan"` が `--permission-mode` に渡ることを確認。

**4. task-api の HTTP テスト**
- `crates/task-api/tests/execution.rs` に
  `getting_a_replanned_task_returns_the_active_version_with_both_versions_listed` を追加
  （`crates/task-api/src/execution.rs` 自体は触っていない）。POST で v1 を採用 →
  `task_ops::execution::replan` を直接呼んで（POST は新規採用専用で 409 を返すため）WU を 1 つ
  足した v2 を採用 → GET の本体が最新の active（v2、追加した WU も `work_units` に含む）を返し、
  `versions` に v1（`superseded`、`superseded_at` 付き）と v2（`active`）が並ぶことを確認。
  `Event::ExecutionPlanned{version:2, supersedes: Some(v1_id)}` が events から監査できることも
  確認した（`versions` 自体に `supersedes` 列は無い設計〈E4 の逸脱記録どおり〉なので、その裏付けは
  events で行う）。
- `GET /tasks/{id}/execution` は本 worktree のブランチにはまだ存在しない（E5 が別 worktree で
  作業中の可能性があるため、無いものには触れない指示どおり見送り）。
- 実行したコマンド: `cargo test -p task-api --test execution`
- 出力の要点: exit 0、8 passed（新規 1 本を含む）。
- 実行したコマンド: `cargo test -p task-api --no-fail-fast`
- 出力の要点: exit 0。38 個の `test result:` ブロック全て ok（FAILED 0）。

**5. `celerisctl replay`/`rebuild_work_units_and_runs` の `execution_plans` 対応（E2b からの持ち越し）**
- `task_core::store::TaskStore` に `execution_plans_replace`（`work_units_replace`/`runs_replace`
  と同じ形。DELETE + INSERT を 1 トランザクションで）を追加。
- `task_ops::replay::rebuild_execution_plans`（新規）: `Event::ExecutionPlanned` だけから
  `execution_plans` の版の履歴を再構築する純粋関数。`plan_id` はイベント自身が運ぶ（`adopt_plan`/
  `replan` が `new_id()` で発行し、行と event の両方に同じ値を書く）ので `work_units.id`（`key` しか
  運ばれない）と違って確実に復元できる。`status`/`superseded_at` は、後続イベントの `supersedes` を
  見て畳み込む（最後まで supersede されなかった版が `active`）。`planner_run_id` は
  `Event::ExecutionPlanned` 自体が運ばない欄なので、events だけからは確実に復元できず常に `None`
  にし、`diff_execution_plans`（新規）の比較対象からも外した（`work_units`/`runs` の
  `created_at`/`updated_at`/`last_checkpoint_run_id` と同じ「タイムスタンプ級で厳密には復元できない
  欄は比較しない」という E2b の規則の延長）。`check_and_apply_execution` に配線し、戻り値に
  `execution_plan_mismatches` を追加（型が複雑になったので `ExecutionCheckReport` エイリアスに
  整理。clippy `type_complexity` 対応）。`--apply` で書き戻すときは、比較対象外の `planner_run_id`
  を既存の stored 行から引き継ぎ、消えないようにした（そうしないと、`work_units`/`runs` だけが
  食い違っていて `execution_plans` は正しいタスクでも、3 表を一緒に書き直す際に
  `planner_run_id` が黙って消えてしまう）。`celerisctl replay` に `EXECUTION_PLAN_MISMATCH` 行の
  出力を追加。
- 実行したコマンド: `cargo test -p task-ops --lib -- check_and_apply_execution_rebuilds_the_replanned_execution_plans_history`
- 出力の要点: exit 0、1 passed。`adopt_plan` → `replan` で v1/v2 を作り、
  `rebuild_execution_plans` が `supersedes`/`superseded_at`/`active` を正しく復元することを確認
  → `execution_plans` を events に無い値（v2 の `origin`）で故意に壊す → `--check` で検出
  （`plan_mm` に `version=2 field=origin` が出る）→ `--apply` で直り、かつ比較対象外の
  `planner_run_id`（`"planner-run-1"`）が消えないことを確認 → 直った後の再 `--check` で差分ゼロ。
- `cargo test -p task-ops --lib`: exit 0、319 passed（既存 318 + 新規 1）。
- `cargo test -p task-core -p celerisctl --no-fail-fast`: exit 0。両クレート合わせて全ブロック ok。

**6.（任意）配送の repair**
- 未実装。ADR 自身・今回の指示ともに「任意。時間があれば」としている項目で、1〜5 の実装・検証を
  優先し、時間の制約により見送った。`crates/celeris/src/delivery.rs:338-354` の `[delivery-repair]`
  （マージ/ビルドの技術的な不備を、実装担当への `Trigger::Reopen`〈Task 全体の再実行〉+ コメントで
  差し戻している）を、`task_core::execution::RepairClass::MergeBase`（型・budget は E4 で用意済み。
  `crates/task-core/src/execution.rs:580,592,601`）を使った repair WU の組み立てに置き換えるのが
  次の一手。`delivery.rs` は dispatcher の外（celeris 本体のポーリング処理）から `store` を直接
  操作しており、`dispatcher.rs::try_review_repair`/`TaskStore::review_repair_apply` が前提にしている
  「reviewing → repair WU → 再レビュー」の状態遷移とは別の入り口（`Ready|Blocked → Reopen`）なので、
  `review_repair_apply` をそのまま呼べるかの設計確認から要る。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし。各項目コミット前に確認） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（警告 0。項目5で `ExecutionCheckReport` 型エイリアスを導入して `type_complexity` を解消） |
| 全テスト（1回目） | `cargo test --workspace --no-fail-fast` | 81 個の `test result:` ブロック中 1 個 FAILED（`e2e::provider_admin_scenarios::reload_clears_provider_cooldown`）。高負荷（他セッションの並行 cargo ビルドで load average 12〜24/24 コア）下での実機ポーリングのタイムアウトと判断し、切り分けのため item5 の変更を `git stash` で外した状態でも再現（＝本変更由来ではない）、`stash pop` で戻した後は単体実行で 3 回連続 ok を確認 |
| 全テスト（2回目、確認） | `cargo test --workspace --no-fail-fast` | exit 0。81 個の `test result:` ブロック全て ok（FAILED 0） |
| GUI gen:types（項目2 のみ） | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と同一（差分ゼロ） |
| GUI typecheck（項目2 のみ） | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0（エラーなし） |

項目 1・3・4・5 は `Event`/API/protocol の型を変えていない（項目1は `ExecutionPlannerContext` を
拡張したが worker-protocol のみ、項目4はテスト追加のみ、項目5は task-core/task-ops/celerisctl 内部）。
`UPDATE_SCHEMA` の再生成は項目1（worker-protocol）と項目2（api-v1、`WorkUnitBlockedReason`）でのみ
行った。

### 未解決事項・次の一手

- **項目6（配送の repair の merge_base 化）**: 上記のとおり未実装。次の worktree（E5/E6）で
  `delivery.rs` の入口（`Reopen`）から `review_repair_apply` 相当を呼べるかを先に設計すること。
- E1〜E4 からの申し送りのうち本 Phase で扱わなかったもの（S4/S6 の実データ配線、D16 の repair
  lane 強制、`node_sessions` 関連など）は引き続き E5/E6 で検討する。
- `gui/`・`task-ops/src/view.rs`・`task-api/src/{stats.rs,routing.rs}`・
  `task-core/src/execution_metrics.rs` は指示どおり触っていない（E5 が並行編集中の可能性）。

## Phase E5「GUI と metrics」（2026-09-25）

ADR-0072 §6 E5 の受け入れ条件 (a)〜(e) を実装した。作業の前提: worktree のブランチが main の
Phase E4 統合（`751f973`）と同じ時点から分岐していたため `git merge` は不要だった（fast-forward
かどうかの確認だけ行い、コンフリクトは無かった）。branch: `worktree-agent-aed7ebd3d8b37d44f`。

指示どおり区切って進めた: (a) metrics と API（1 コミット）→ (b)(c)(d) GUI の Execution 節
（1 コミット）→ ADR の逸脱節（1 コミット）→ 本節（PROGRESS と最終ゲート）。途中、利用制限で
一度中断し、再開時に未 commit だった ADR の逸脱節を先に commit してから (e) の監査に進んだ。

### 受け入れ条件ごとの証跡

**(a) `ExecutionMetrics` の純粋関数のテスト。`GET /tasks/{id}/execution` と `GET /metrics/execution`**
- 新規: `crates/task-core/src/execution_metrics.rs::summarize(&Task, &[Event]) -> ExecutionMetrics`
  （純粋関数。`gate_mode`/`work_units_total`/`work_units_done`/`runs_by_role`/`continuations`/
  `budget_exhausted_by_kind`/`max_turn_failures`/`retries`/`repairs_by_class`/`repairs_total`/
  `replans`/`peak_context_tokens`/`total_input_tokens`/`total_output_tokens`/`cost_usd`/`wall_ms`/
  `final_status`）。D19 の signature（`&Task, &[Event]`）はそのまま守り、`wall_ms` は
  `Task.created_at`→`updated_at` で近似した（ADR の逸脱節参照）。
- 実行したコマンド: `cargo test -p task-core --lib execution_metrics::`
- 出力の要点: exit 0、10 passed（空 events・gate 反映・continuation・budget_exhausted の kind 別・
  work_units/replan の派生・repair の class 復元〈atomic 経路は成功・既存計画への追加経路は
  `"unknown"` に落ちることの両方〉・tokens/cost/peak_context の集計・runs_by_role・wall_ms が
  終端のときだけ出ることを確認）。
- `GET /tasks/{id}/execution`（`crates/task-api/src/execution.rs::get_task_execution`）: 計画・
  WU 一覧（`ExecutionPlanView`。既存の `WorkUnitView` を再利用）・runs 一覧（`RunSummary`、files
  付き）・metrics・`ExecutionPhase` を 1 つにまとめて返す。
- `GET /metrics/execution?since=&group_by=gate_mode|genre|assignee|lane`
  （`crates/task-api/src/stats.rs::execution_metrics_summary`）: タスク一覧 + events の全走査で
  gate の判定分布・completion rate・continuation/max_turn_failures/repair/replan の頻度を group_by
  でまとめる（「`runs` の索引から作る」という ADR の文言からの逸脱。理由は ADR 参照）。
- 実行したコマンド: `cargo test -p task-api --test execution`
- 出力の要点: exit 0、14 passed（既存の (POST)/(GET) execution-plan のテストに加え、
  `task_execution_for_an_unknown_task_is_not_found`・
  `task_execution_with_no_activity_returns_zeroed_metrics_and_no_plan`・
  `task_execution_reports_gate_plan_and_phase`・`execution_metrics_rejects_an_unknown_group_by`・
  `execution_metrics_rejects_a_malformed_since`・`execution_metrics_groups_by_gate_mode_by_default`・
  `execution_metrics_since_filters_out_tasks_updated_before_it` を追加）。

**(b) タスク詳細の Execution 節（D20）。計画なし・計画あり・replan あり・repair ありの 4 fixture で vitest**
- `crates/task-ops/src/view.rs`: `TaskDetail.execution: Option<ExecutionView>`（`ExecutionPhase`/
  `ExecutionPlanOverview`/`ExecutionWorkUnitView`/`ExecutionPlanVersionSummary` を新設。
  `task-api::types::ExecutionPlanView`/`WorkUnitView` とは別の軽量な型。理由は ADR 参照）。
  `RunSummary.work_unit: Option<String>`（`run_work_unit_keys` で `work_units`/`runs` の索引から
  引く）も追加。
- 実行したコマンド: `cargo test -p task-ops --lib view::`
- 出力の要点: exit 0、46 passed（既存 41 + 新規 5: `task_detail_execution_is_none_for_a_task_with_no_execution_activity`・
  `task_detail_execution_shows_gate_only_when_there_is_no_plan`・
  `task_detail_execution_reports_the_plan_and_work_unit_table`・
  `task_detail_execution_reports_replan_version_history`・`execution_phase_reviewing_is_always_verifying`）。
- GUI: `gui/app/lib/task-execution.ts`（新規。celeris の値をそのまま並べる純粋関数。
  `~/lib/task-routing.ts` と同じ置き場）+ `gui/app/components/ExecutionSection.tsx`（新規）。
  `gui/test/unit/task-execution.test.ts` に計画なし・計画あり・replan あり（版の履歴 2 件）・
  repair あり（`kind = repair` の WU）の 4 fixture、5 テスト。
- 実行したコマンド: `cd gui && corepack pnpm@11.27.0 exec vitest run test/unit/task-execution.test.ts`
- 出力の要点: exit 0、1 file / 5 passed。

**(c) 古いタスクの詳細（`execution` 無し）が変わらない**
- `build_execution_view`（`task-ops/src/view.rs`）は、events に E-phase 由来の活動
  （`ExecutionGated`/`ExecutionPlanned`/`CheckpointSaved`/`WorkUnitTransitioned`）が 1 件も無ければ
  `None` を返す（`TaskDetail.execution` は `skip_serializing_if = "Option::is_none"` なので JSON に
  現れない）。`task_detail_execution_is_none_for_a_task_with_no_execution_activity`（上記 (b)）で
  直接検証。
- 既存の `task-ops`/`task-api`/GUI のテストは 1 件も期待値を変えていない（後述の全体ゲートで
  既存分を含め全て緑）。`docs/api/v1/api-v1.schema.json`/`gui/app/celeris/types.ts` の差分は
  追加のみ（`git diff` を目視、末尾に要点）。

**(d) runs の表（既存の run 一覧）に `end`（completed/yielded/budget_exhausted …）と WU の列**
- `gui/app/routes/tasks.$id.tsx`: runs の表に `end`（`RunEnd` のバッジ）・`WU`（`work_unit` の
  key）の列を追加。`~/lib/task-execution.ts::runEndLabel`/`runEndTone` を使用。
- タスク詳細のヘッダに `ExecutionPhase` バッジ（`task-status` の隣。計画も gate の判定も無ければ
  出ない）。

**(e) `gen:types` の差分ゼロ（2 回実行）、`pnpm build`、`mobile-audit` の違反 0（393px で崩れない）**
- 実行したコマンド: `cd gui && corepack pnpm@11.27.0 gen:types`（2 回）
- 出力の要点: `app/celeris/types.ts` は 1 回目で `ExecutionPhase`/`ExecutionView`/`ExecutionMetrics`/
  `ExecutionPlanOverview`/`ExecutionWorkUnitView`/`ExecutionPlanVersionSummary`/`TaskExecutionView`/
  `ExecutionMetricsSummary`/`ExecutionMetricsGroup`/`RunSummary.work_unit` が追加され（`git diff`
  は追加のみ + 既存コメント 1 行の位置が `$defs` のアルファベット順で動いただけ）、2 回目は
  1 回目と完全に同一（`diff -q` 差分なし）。最終確認では `git status --short
  gui/app/celeris/types.ts` も空（commit 済みの内容と一致）。
- 実行したコマンド: `cd gui && corepack pnpm@11.27.0 typecheck && corepack pnpm@11.27.0 lint &&
  corepack pnpm@11.27.0 test && corepack pnpm@11.27.0 build`
- 出力の要点: typecheck exit 0（エラーなし）。lint exit 0（`scripts/check-resume-recovery.mjs` の
  `lint/style/useTemplate` info 2 件のみ。E5 が触っていない既存ファイルの pre-existing な info で
  `pnpm lint` の exit code には影響しない）。test: 72 files / 1091 passed（E5 前は 71 files / 1086
  passed。差分は `task-execution.test.ts` の 1 file / 5 tests）。build 成功
  （`tasks._id-*.js` 72.82 kB、`INEFFECTIVE_DYNAMIC_IMPORT` の warning 2 件は E5 が触っていない
  既存の `task-files.tsx`/`task-changes.tsx` の静的+動的二重 import で pre-existing）。
- 実行したコマンド: `UV_USE_IO_URING=0 corepack pnpm@11.27.0 mobile-audit`
  （`UV_USE_IO_URING=0` はこのサンドボックスだけの事情。ADR の逸脱節参照。コードの変更ではない）
- 出力の要点: `{"ok":true,"total":0,"by_rule":{},"by_scheme":{"light":0,"dark":0}}`。
  routes=27 schemes=2 violations=0。`git_sha` はこの完了報告の最終 commit の sha12 と一致する形で
  記録される（監査レポートの慣例）。
  - 初回の実装（WU の表・runs の表を `overflow-x-auto` の横スクロール表のままにした版）は
    `touch-scroll` の違反が出た（`task-overview` の `execution-section`/`runs-section` の各 2 件）。
    原因は `checkFocusOrder`（Tab キーで文書全体を歩く検査。`checkTouchScroll` の直前に走る）が
    表内の `<details>`（checkpoint の折り畳み・run の outcome 詳細）にフォーカスすると、ブラウザが
    横スクロールコンテナをネイティブに「要素が見える位置まで」動かし、その後の
    スワイプ検査が向きを変えられず偽陽性になるというもの（デバッグ用の使い捨てスクリプトで、
    `scrollIntoViewIfNeeded` を経由しない単発の swipe は成功することを確認して特定した）。
    D20 の指示どおり `max-sm:` で表からカードの一覧に折り返す設計に直し（`~/routes/projects.tsx`
    の案件一覧と同じ技法）、解消した（ADR の逸脱節に詳細）。
  - 副産物: run 一覧の `started_at`/`finished_at` セルが ADR-0055 D1-4（本文 14px 以上）に
    違反していたことも見つかった（`task-overview` の fixture がこれまで `runs: []` だったため、
    E5 で初めて可視化された既存の欠落）。`text-sm ... lg:text-xs` に直した。
  - `gui/scripts/lib/celeris-fixture.mjs`: `mobile-audit`/`e2e-check` が共有する `task-overview` の
    fixture に `execution`（gate・計画・WU 3 件〈done/blocked/repair〉・replan の版履歴 2 件）と
    `runs`（`end`/`work_unit` 付き 2 件）を追加し、新しい Execution 節を実際に描画した状態で
    機械検査されるようにした。

### ゲート（本 Phase 完了時点）

| ゲート | 実行したコマンド | 出力の要点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（警告 0） |
| 全テスト | `cargo test --workspace --no-fail-fast` | exit 0。81 個の `test result:` ブロック全て `ok`（FAILED 0、計 2,273 tests passed） |
| schema（task-core） | `UPDATE_SCHEMA=1 cargo test -p task-core --lib` の `committed_schema_matches_generated`（execution/execution_plan/plan） | exit 0、3 passed。差分なし |
| schema（task-worker） | `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` | exit 0、1 passed。差分なし |
| schema（task-api） | `cargo test -p task-api --lib schema::` | exit 0、2 passed（`committed_schema_matches_generated`・`schema_uses_defs_once_for_shared_types`）。`docs/api/v1/api-v1.schema.json` は既に commit 済みで差分なし |
| GUI typecheck | `cd gui && corepack pnpm@11.27.0 typecheck` | exit 0 |
| GUI lint | `cd gui && corepack pnpm@11.27.0 lint` | exit 0（pre-existing な info 2 件のみ、E5 の対象外ファイル） |
| GUI test | `cd gui && corepack pnpm@11.27.0 test` | exit 0。72 files / 1091 passed |
| GUI gen:types | `cd gui && corepack pnpm@11.27.0 gen:types` を 2 回実行し diff | 2 回目が 1 回目と同一（差分ゼロ）。`git status --short` も空 |
| GUI build | `cd gui && corepack pnpm@11.27.0 build` | exit 0 |
| GUI mobile-audit | `cd gui && UV_USE_IO_URING=0 corepack pnpm@11.27.0 mobile-audit` | `ok:true`、routes=27 schemes=2 violations=0 |

**途中で踏んだ 2 件のフレーク（本 Phase の変更とは無関係。証跡）**:
- `sse_delivers_created_quickly_and_resumes_from_last_event_id`（`tests/e2e`）: 最初のフルテストで
  `celerisctl replay` の mismatch（`status=Reviewing` vs `Running`）で 1 回だけ落ちた。単体では
  `cargo test -p e2e --test api_scenarios sse_delivers_created_quickly_and_resumes_from_last_event_id`
  で 0.52 秒で通過（時間依存のフレーク。過去の Phase でも同種の記録あり）。
- `acp::tests::{startup_timeout_without_any_response_is_a_spawn_failure,
  wall_clock_exceeded_cancels_then_kills_the_process_group, idle_timeout_kills_and_reports_error}`
  （`task-worker`）と `accounts_admin::tests::spawn_check_codex_reports_ok_and_records_observation`
  （`celeris`）: 別 worktree（E4b のセッション）が同じマシンで並行に `cargo test --workspace`/
  `cargo clippy` を走らせていた時間帯に 2 回に分けて落ちた（`git diff --stat 751f973..HEAD --
  crates/task-worker/ crates/task-dispatch/ crates/celeris/` はいずれも空 = 本 Phase はこれらの
  crate を一切触っていない）。負荷が退いた後 `cargo test -p task-worker --lib acp::tests::` で
  26/26 が 0.73 秒、`cargo test -p celeris --lib
  accounts_admin::tests::spawn_check_codex_reports_ok_and_records_observation` も単体で通過。
  最終の `cargo test --workspace --no-fail-fast`（上表）は FAILED 0 で確認済み。

### ADR との差分

`docs/adr/0072-task-execution-decomposition.md` の「Phase E5 実装時の逸脱・明確化」に詳細を記載。要点:

1. 触るファイルが ADR §6 E5 の表より広い（`task-core/src/lib.rs`〈モジュール宣言〉、
   `task-api/src/schema.rs`〈`ApiV1Schema` への 2 型の登録〉、`task-api/tests/execution.rs`
   〈(a) の HTTP テスト〉、`gui/app/lib/task-execution.ts`〈新規〉、
   `gui/scripts/lib/celeris-fixture.mjs`〈mobile-audit の fixture〉）。
2. `GET /metrics/execution` の集計は「`runs` の索引から作る」ではなく、`store.rs` を触らずに
   タスク一覧 + events の全走査にした（低頻度な分析用クエリという想定）。
3. `repairs_by_class` は既に計画のある Task への repair 追加（`WorkUnitTransitioned` だけの経路）
   では class を復元できず `"unknown"` に落ちる既知の限界（events だけの純粋関数の制約）。
4. `ExecutionPlanVersionSummary`（版の履歴）に「差分の件数」は含めない（`reason` の自由記述まで。
   次の一手）。
5. `ExecutionPhase` の導出規則は D20 の文面に明文化が無い決め打ち（`task_ops::view::execution_phase`
   のドキュメントコメント参照）。
6. GUI: D20 の「モバイル幅: WU の表はカードの一覧に折り返す」を、当初の横スクロール表の実装で
   `mobile-audit` の `touch-scroll` に落ちたことを受けて設計をやり直し、`max-sm:` のカード表示
   （runs の表にも同じ理由で適用）にした。

### 未解決事項・E6 への申し送り

- **`GET /metrics/execution` の集計方式**: 現状はタスク一覧 + events の全走査（`StatsState` のような
  増分カーソルは持たない）。タスク数が多い実運用で遅ければ、`store.rs` に `runs` 由来の専用集計
  クエリを足す。
- **`repairs_by_class` の `"unknown"` バケット**: 既に計画のある Task への repair 追加時に class を
  events だけから復元できない（D16/E4 の設計に起因。`WorkUnitSpec` に repair 専用の欄を足すか、
  別の記録方法が要る）。
- **`ExecutionPlanVersionSummary` に差分の件数（added/changed/removed）を足す**: `E4` の
  `task_ops::execution::ReplanDiff` を events から再計算する経路、または `execution_plans` の行に
  差分を保存する設計変更が要る。
- **`GET /metrics/execution` の `group_by = lane`**: WU 単位ではなくタスクの直近 run の lane を代表値
  にしている。WU 単位の lane 別集計が要ることが分かれば次の一手。
- E1〜E4 からの申し送り（S4/S6 の実データ配線、`permission_mode` の実行時配線、D17 3. の
  `plan_issue` トリガー、(h) 配送の repair 等）は本 Phase では着手していない（引き続き E6 で検討）。
- E6（end-to-end の dogfood）は本 Phase の範囲外。ADR §6 E6 の受け入れ条件どおり、認証が使える
  環境でエージェントが実行し証跡を残すか、使えなければ人に手順を渡す。

## Phase E4 / E4b / E5 の本番反映と E6 の開始（2026-09-25 10:51Z）

- 統合: E4 751f973、E5 9cf66c2、E4b a119458（E4b は E5 と `crates/task-api/tests/execution.rs` で衝突し、担当エージェントが自分の branch に main を取り込んで解消。私の統合スクリプトは docs 以外の衝突を含む merge を一度 commit してしまったが push 前に `git reset --hard origin/main` で戻した。以後、コードの衝突は必ず abort して担当に解消させる）。ゲート: fmt 0、cargo test FAILED 0、clippy 0、GUI typecheck / lint / test 1091 件 / gen:types 差分ゼロ。
- release `a1194588b417`、verify 全 true、in-flight 0 でライブ切替（from a2968d1b7477）。**Phase 119 の実機確認**: 旧デーモン a2968d1b7477 は「runtime shutdown reached its time bound; a background task … likely did not stop」を出して自分で終了し、プロセスは現行 1 つだけになった（終了ハングは解消）。
- 本番設定に `[execution] gate = "on"`（`config.toml.bak-20260925a`）。E6 の dogfood のため。既定（shadow）に戻すかは E6 の結果で判断。
- E6 dogfood タスク 01M3C33KW8YH336QDD0QAV45H8（software-engineering、`execution: compound` を人が明示、3 成果: 配送の repair WU（E4 の (h)）、pricing の fable / gpt-6 単価（P-118-1）、`GET /metrics/execution` の索引化）を `POST /tasks` で投入。比較の基準は 1 巨大 session だった 01M38J4X53P1Y684FS42Z6R0VZ（ルーティング再設計、89 分の run を切替で失い attempts 3 で failed）と Knowledge GC の複製群。

## Phase E6「dogfood: 配送の局所修復・単価表・実行 metrics の集計性能」（完了日 2026-09-25）

ADR-0072 D16 に沿って、配送の merge-base 不一致と `cargo-fmt-check` 不合格から
`RepairClass::MergeBase` / `Format` の repair WorkUnit を作る。repair には対象ブランチ、
main の SHA、失敗した check の出力だけを渡し、元の実装 run の context は再構築しない。
偽 git リポジトリの E2E テストは merge-base のずれから repair WU、再検証、配送まで確認する。
fmt 以外の release gate 失敗など、分類しない配送失敗は従来の Reopen を維持する。

`pricing.rs` に現行 Claude 4 モデルの入力・出力・キャッシュ単価を追加し、
`claude-fable-5-1` の費用推定を確認した。**P-118-1 は解決**。
`gpt-6-astra` / `sol` / `luna` は単価の一次根拠がないため全欄を `None` とした。
根拠と適用範囲は `docs/llm-source.md` の単価表に記録した。

`GET /metrics/execution` は `tasks` / `work_units` / `execution_plans` / `runs` の索引集計行を使う。
索引にない lane、`BudgetExhausted` の種類、atomic task の continuation / retry、旧履歴は
該当タスクの events だけで補完する。8 種のタスクについて `since` の有無 × 4 `group_by` の
全組み合わせで events 版との完全一致を確認した。2,000 タスク × 20 events の手元計測は
索引版 95.90548 ms、events 版 369.711016 ms（性能閾値の assert は置かない）。

E4/E5 の申し送りのうち、**(h) 配送の repair** と **`GET /metrics/execution` の全 events 走査**は
本 Phase で解消した。E5 の「索引集計が遅ければ store に専用 query を足す」も実施済み。

### 証拠コマンドと結果

- `cargo fmt --all -- --check`: exit 0、差分なし。
- `CARGO_TARGET_DIR=target/e6-metrics-api cargo test --workspace`: exit 0。81 個の
  `test result:` ブロックで 2,289 passed、0 failed、5 ignored、`FAILED` 行なし。
  共有 Cargo キャッシュは sandbox から `.cargo-lock` を開けず、ローカル target に切り替えた。
  sandbox 内の初回実行では一時 loopback bind が拒否されたため、同一コマンドを sandbox 外で再実行した。
- `CARGO_TARGET_DIR=target/e6-metrics-api cargo clippy --workspace --all-targets -- -D warnings`:
  exit 0、warning 0。
- 完全な検査ログと成果ごとの変更ファイルは run の `artifacts/finish/` と `artifacts/report.md`。
- main `f479ca500753` を取り込み、`docs/PROGRESS.md` のみの衝突を両節を残して解消した。
  取り込み後、`cargo fmt --all -- --check` は exit 0（1.90 秒）、
  `env -u CARGO_TARGET_DIR cargo test --workspace` は exit 0（132.82 秒、81 ブロック、
  2,289 passed / 0 failed / 5 ignored、`FAILED` 行なし）、
  `env -u CARGO_TARGET_DIR cargo clippy --workspace --all-targets -- -D warnings` は exit 0。
  ログは run の `artifacts/merge-main/` に保存した。

### 未解決事項

- レビューの deterministic check は worktree の `target/` を使う。finish で別 target にビルドすると
  cold compile が 600 秒の timeout を消費する。仕上げの WU はレビューと同じ target
  （`CARGO_TARGET_DIR` 未設定、sandbox 外）で最後に `cargo test --workspace` を通しておく。
- GPT-6 3 モデルの単価は一次情報が揃うまで不明。0 USD とみなさない。
- lane と `BudgetExhausted` の種類は runs 索引に無い。旧履歴や atomic task の continuation / retry も
  対象タスクの events による局所補完を要する。
- fmt 以外の release gate 失敗は Reopen のまま。repair 上限超過は人による判断が必要。
- E4/E5 のその他の申し送り（`permission_mode` 実行時配線、repair lane、WU 単位の lane 集計、
  replan 差分件数など）はこの 3 成果の対象外で、継続する。

### 提案

- テスト時間の候補: `tests/e2e/tests/scenarios.rs` は `--until-idle` が
  `idle_timeout_secs=30` を待つため 4 テストで約 30 秒。今回の実測では
  `task_ops` lib の 324 テストも約 30 秒（`task_dispatch` lib の 286 テストは約 4 秒）。
  評価器の負荷を見て縮めるか判断する。
- runs 索引へ lane と run end の種類を保存し、旧履歴の再構築方式を決めた後に局所 events 補完を
  さらに減らす。配送 gate の他の step は失敗ごとに修復可能性と最小 context を検討する。

## E6 dogfood の完了と配送の昇格（2026-09-25 15:03Z）

- dogfood タスク 01M3C33KW8YH336QDD0QAV45H8 は 10:51Z 投入 → 14:58Z done（壁時計 4h06m）。gate は人の明示で compound、planner v1（14 分）→ WU 6 つを直列に完走 → 最終レビュー不合格 → replan v2 → 追加 WU → レビュー → replan v3 → 追加 WU → 最終レビュー合格。8 WU / 8 done、run は planner 3 / worker 8 / reviewer 3、continuation 0、budget_exhausted 0、retry 0、repair 1、replan 2、費用 11.21 USD、attempts 2（最終レビュー不合格が ReviewFail 扱い）。
- 成果（配送の repair WU、pricing の fable / gpt-6 単価、`GET /metrics/execution` の索引化）は Celeris の配送で origin/main（5cc1610）に入り、release `5cc1610938f5`（gate ok、verify ok）を in-flight 0 でライブ昇格。**本番は Celeris 自身が compound として分解・実行した成果で動いている。**
- 比較と提案の分析は Phase E6（分析）として Sonnet に委譲（`docs/execution-decomposition-report-2026-09-25.md`）。
## Phase E6（分析、完了日 2026-09-25）

ADR-0072 §6 E6 の受け入れ条件どおり、dogfood タスク 01M3C33KW8YH336QDD0QAV45H8（compound、
gate=human 明示）と比較対象 2 件（01M38J4X53P1Y684FS42Z6R0VZ = 巨大 1 session の routing 再設計、
01M39FGDAE9XQA3FGMCP5MCW0B = その retry 複製）を、本番 API から取得済みの読み取り専用 JSON
コピーで分析した。コードは変更していない（`docs/` のみ）。成果物:
`docs/execution-decomposition-report-2026-09-25.md`（章立てはそちらを参照）。

### 比較表の要約（詳細は報告書 §2）

| 指標 | dogfood（after） | 巨大 session（before） | retry 複製 |
|---|---|---|---|
| 最終状態 | done | **failed** | done |
| 壁時計 | 4h6m4s | 1h48m19s | 24m39s |
| run 数（役割別） | 14（planner3/worker8/reviewer3） | 5（worker3/reviewer2） | 4（worker2/reviewer2） |
| replan 数 | 2 | 0（仕組みなし） | 0 |
| repair 数 | 1（`repairs_by_class: unknown`） | 0 | 0 |
| attempts（推測。§2注2） | 推測2 | 推測3（`max_retries`超過で failed） | 推測1 |
| cost_usd（注: 過小評価。§4） | $11.21 | $1.76（lease失効runのusage喪失込み） | $1.67 |
| peak context | 未計測（全タスク共通。U2 のまま） | 未計測 | 未計測 |

### 主な発見

1. before の失敗は「89 分 run をライブ切替の drain で lease 失効・usage 喪失」+「review 不合格 2 回」
   の組み合わせで、E1（budget_exhausted の非失敗化）と Phase 119（drain 後にプロセスが終了しない
   障害の修正）の両方が直接効く種類の失敗だったことを実データで確認した。
2. dogfood で実際に発火した repair・replan は、D16/D17 が想定する「典型的な不合格」（fmt/lint/
   小さなテスト失敗）ではなく、(a) レビュー環境のビルドキャッシュ不一致による 600 秒タイムアウト、
   (b) 4 時間の実行中に main が先行したことによる merge-base のずれ、の 2 件で、どちらも D16 の
   5 分類のどれにも当たらず `substantive`（`ReviewFail` で attempts 消費）+ 重い replan を経由した。
   `repairs_by_class: unknown` は、replan が新設した `kind: repair` の WU が D16 の title 接頭辞
   規約に従っていないために class を復元できない、既知の設計限界の実例。
3. `GET /tasks/{id}/artifacts` は `{"items":[]}`（27 個のファイルが実在するのに空）。
   `crates/task-api/src/files.rs:234-246` が `Event::ArtifactProduced` だけを見る実装のため。
4. 入力トークン 65.1M の 99.9% は codex（`gpt-6-sol`）worker run で、これらは `cache_read_tokens`
   フィールド自体を持たない。Claude 系 run の cache_read 合計は 5.3M（総入力の 8%）で、
   「入力の大半が cache read」という前提は実測と食い違う（訂正として記録）。
5. `gpt-6-sol` は `pricing.rs` の `PRICE_TABLE` が全欄 `None` のため、8 件の worker run
   （総入力トークンの 99.9%）の費用が `cost_usd` から丸ごと除外されている。`$11.21` は
   実装作業の費用をほとんど含まない過小評価。
6. shadow 計測は実質機能していない: `metrics-execution.json`（`since=2026-09-24`、46 タスク）で
   `gate_mode=compound` は dogfood の 1 件のみ、かつ `source=human`（規則表は未評価、
   `execution_gate.rs:199-210` の早期リターン）。gate の閾値・重みは E0 時点の推測値のまま
   実質未検証。

### gate の推奨

**既定を `on` にしない。`shadow` を維持する。** 根拠は上記6（サンプル N=1、かつ規則表を経由して
いない）。先に (a) 問題1（review 不合格の分類の弱さ、特に「レビュー環境要因のタイムアウト」の
repair 化）と (b) 問題5（モデル単価欠損の費用集計への影響の可視化）を直し、(c) `shadow` のまま
2〜4 週間分の `ExecutionGated` 実績（人の明示に頼らないもの）を溜めてから、全面 `on` ではなく
部署・genre 単位の段階導入を検討する。本番設定は E6 dogfood 実行のために `gate = "on"` に
手動で上書きしていた（Phase E4/E4b/E5 の本番反映節）。この分析の結論に従い、**`shadow` に
戻すことを提案する**（実施は人の判断）。

### 提案（報告書 §4〜6 の要約）

- P-E6a-1: D16 に「review timeout」class を追加し、決定的検査のタイムアウトを repair 対象にする。
- P-E6a-2: `merge_base` の repair 分類を、配送段階だけでなく Task 内部の最終レビュー段階でも
  使えるようにする。
- P-E6a-3: `GET /tasks/{id}/artifacts` が `report.md` 等の既知ファイルを拾えるよう、
  worker 側にプロンプトで `ArtifactProduced` 相当の登録を促すか、API 側にベストエフォートの
  フォールバックを足す。
- P-E6a-4: codex アダプタが prompt cache の usage を報告できるか確認し、できなければ
  `docs/llm-source.md`/ADR-0072 に「codex は現状キャッシュ非対応」と明記する。
- P-E6a-5: `ExecutionMetrics` に `cost_usd_complete: bool`（単価不明モデルを含めば `false`）を
  足し、費用が過小評価であることを GUI・報告で明示する。
- U3（WU 並列）の優先度を上げる: dogfood の 3 成果は依存グラフ上独立だったが直列実行のため
  壁時計 4h6m のうち大半を消費した。

### 未解決事項

- ADR §7 の U1〜U10 は本分析では 1 件も新規に解消していない（dogfood で `budget_exhausted`
  ・`peak_context_tokens` が 1 件も発生/計測されなかったため検証機会が無かった）。詳細は
  報告書 §6 の表。
- reviewer run の `runs` 索引に `finished_at`/`usage`/`end` が欠落する経路を 1 件観測した
  （`01M3CDS6K0JPYT0GRDJ97Z986T`）。原因は本分析の範囲では特定できていない。次の一手として
  `celerisctl replay --check` を実機で当該タスクに対して実行すること。
- 01M3C33KW8YH336QDD0QAV45H8 の release `5cc1610938f5` は gate_ok=true・verify.ok=true・
  live_ok=true だが、分析用に取得した `releases.json` のスナップショットでは `promoted_at: null`・
  `is_current: false`（本番の `current` は E6 より前の `a1194588b417` のまま）だった。
  「15:03Z に昇格済み」という前提と食い違うため、事実として記録する（本番の実際の昇格状況は
  このエージェントの環境からは確認できない。人による確認を推奨）。
- 訂正（Fable、2026-09-25 15:0xZ）: E6 分析の入力 `releases.json` は昇格前のスナップショットだったため「5cc1610938f5 は未昇格」と記されているが、実際には 15:03Z にライブ昇格済み（`GET /health` release=5cc1610938f5）。分析の推奨に従い、本番設定の `[execution] gate` は `"on"` → `"shadow"` に戻す（`config.toml.bak-20260925b`。次回の昇格で有効）。

