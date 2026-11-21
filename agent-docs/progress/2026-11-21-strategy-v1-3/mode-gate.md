---
title: 戦略層 v1 子 3 mode-gate — discussion mode の記録と /cos/operations の拒否
tasks: [01M7XQ9QE8STND208NXG90F743]
status: blocked
updated: 2026-11-21
---
# mode-gate（WorkUnit。子 3 の段 gate）

状態: blocked（範囲の check が ADR と矛盾するので計画の問題として申告した。下の「Run #2」）。設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D1・D2・D4・D7。
基準は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## したこと

- `POST /chat/threads/{t}/messages`・`/relays`・MCP `console_instruct` に任意の欄 `discussion_mode`（`discuss`・`propose`・`execute`）。
  `discuss`・`propose` だけを `chat_messages.metadata_json.discussion_mode` に書く。省略と `execute` は何も書かない（応答・metadata・chat_events は今と同じ）。
- run の mode の導出 `task_core::discussion_mode::discussion_mode_of_run`（保存しない。入力の窓の最も厳しい mode と、決めた message の `{message_id, author}`）。
- `crates/task-api/src/cos/discussion_gate.rs`: discuss・propose の run は登録済みの操作と `/cos/inbox/{i}/resolve` をすべて 403 `discussion_mode_forbidden`。
  `bind_run_policy` が policy の版の一致の後に呼ぶので、`instructed_by` の解釈より前。未登録の path は今の 422 のまま。拒否は `OperationAudit::reject` で rejected の行と監査 event（payload に mode と decided_by）。
- CoS の prompt（`task-worker` の `cos_chat.rs`）に mode の 1 行（hint）。dispatcher は `cos_context::discussion_mode` で同じ関数を呼ぶ。
- docs/api/v1 の schema・api.md、web の生成型、docs/protocol の worker schema を再生成した。

## ADR からずれた点（理由つき）

- 導出の関数の置き場所を ADR D1.2 の `task_core::chat` から `task_core::discussion_mode`（新しい top-level module。`chat` へ一方向に依存）へ移した。
  `chat` の module に置くと repo health の `size.responsibility-growth` が落ちた（`chat` の本体の行 8092 > 8000、`chat::SqliteStore` の method 115 で増加）。enum `ChatDiscussionMode` は `chat` に残した。
- 依頼の objective (3) は「propose では未承認の案の記録の op だけ許可」と書くが、ADR D2.1・D3 は propose も operation を全拒否とし、案は返事の添付（summary-cite の WU）で残すと決めた。ADR に従った。approved の案の拒否は summary-cite の試験 `discussion_mode_propose_refuses_approved_state_draft` が受け持つ。
- skill cos-operator に discussion mode を prompt だけで守る記述は無かった（consult.md の「task を起票しない」は intent の型の hint で、ADR D1.2 が別物とした）。skill は変えていない（変えると `metadata.version` = policy_version に触れる）。
- health の行数を増やさないため、`launch_claimed` の `harness_capabilities` の match を 2 腕にまとめた（挙動は同じ: `claude_code`・`claude-code` → `claude-code`、`codex` → `codex`）。

## 試験（`discussion_mode_` で始まる試験 18 件と `chat_discussion_mode_` 4 件、すべて通る）

- task-api `cos_delivery_suite_c::discussion_mode`（15 件）: `discuss_task_create_is_403_and_creates_nothing`・`absent_post_is_unchanged`・`unknown_value_is_422`・
  `discuss_denies_every_registered_op`・`propose_denies_every_registered_op`・`discuss_instructed_by_cannot_waive`・`discuss_roadmap_propose_is_403`・
  `discuss_project_update_is_403`・`discuss_publish_ops_are_403`・`external_text_cannot_lift_mode`・`actor_mcp_input_is_named_and_only_restricts`・
  `discuss_inbox_resolve_is_403`・`reject_audit_names_deciding_messages`・`execute_matches_absent`・`actor_relay_without_mode_is_execute`
  （全件 `discussion_mode_` 始まり）。
- task-api `strategy_gate::tests::discussion_mode_execute_unapproved_strategy_is_422`（execute は `strategy_unapproved`、同じ要求を discuss にすると 403）。
- task-core `chat::discussion_mode_tests`（5 件）: `chat_discussion_mode_recorded_only_for_discuss_and_propose`・`_run_takes_strictest_input`・`_system_input_run_is_execute`・`_unreadable_record_is_discuss`・`discussion_mode_relay_and_mcp_instruct_carry_mode`。
- task-worker `cos_chat::tests::discussion_mode_prompt_names_discuss_and_propose_runs`。
- 守る不変条件: discuss・propose の run は `cos_operations` に applied を増やさず、tasks・events（監査以外）・projects・Project.request・roadmap の版・decisions・org・grants の行数と docs・knowledge の file が変わらない。既存の試験は mode が無い時代の挙動しか見ないので、この拒否を検出できない。

## 変更前に落ちた出力（負の試験）

Run #1 で、同じ試験 file（JSON と SQL だけを使う）を base の src に対して実行した（`cargo test -p task-api --test cos_delivery_suite_c discussion_mode`、ログは artifacts の `base-negative.log`、101 行）。

- `test result: FAILED. 1 passed; 10 failed`、exit=101。
- 送信の試験は 400 `unknown field \`discussion_mode\``（base の `deny_unknown_fields`）。
- `discussion_mode_discuss_task_create_is_403_and_creates_nothing` は store に discuss を直接置いても base が task を作った: `left: 200 right: 403`、`tasks=1`。
- 通った 1 件は `discussion_mode_actor_relay_without_mode_is_execute`（回帰の確認で、base でも通るのが正しい）。

## 検査（提出時の HEAD で実行）

- `bash scripts/dev/check-health.sh`: verdict=pass、new=0、worsened=0（初回は 7 件の fail。上の「ずれた点」で直した）。
- `bash scripts/dev/test-affected.sh`: nextest 8601 件のうち 8599 passed・1 failed・1 timed out・31 skipped。web の vitest 705 passed、tsc 通過、test-ts-verify 20 passed。
  - failed 1 件 `task-worker protocol::tests::committed_schema_matches_generated` は worker の schema の再生成漏れ。`UPDATE_SCHEMA=1` で再生成し、単独で pass。
  - timed out 1 件 `celeris-campaign::dagbench_process recovery_notice_continuation`（600 s）は単独の再実行で pass（22 s）。全体実行の負荷下の timeout。
  - 修正後の test-affected の全体の再実行はしていない（時間切れ）。
- `cargo clippy --workspace -- -D warnings`: exit 0（vendor の deno_core の既存の警告 1 件のみ）。
- `cargo fmt --all -- --check`: exit 0。

## migration・表・trait・Manager

足していない。足した型は enum `ChatDiscussionMode`・構造体 `RunDiscussionMode`・`ChatDiscussionDecider` と要求の欄。

## Run #2（2026-11-21）: 前回の check の不合格と対応

前回の run の後の check は 2 つとも不合格だった。

### 1. `test-affected && clippy && fmt`（exit 1）

- 落ちたのは web の unit 試験 `e2e/support/fake-daemon.test.ts` の「records JSON requests, sends SSE immediately, and marks aborted requests」1 件（705 件中）。
  test-affected は web を nextest より先に走らせるので、この check では nextest・clippy・fmt まで進んでいない。
- 原因はこの WU の変更ではない。この WU が web で変えたのは生成物 `web/api/generated/schema.json`・`types.ts` だけで、試験は単独で 3 回とも 17 passed。
  試験は abort の前後を固定の 30 ms の `setTimeout` で待っていた。負荷下では、server が要求を受ける前か close を記録する前に判定して落ちる。
- 直した: 固定の待ちをやめ、`requests` に `/inbox` が記録されるまで待ってから abort し、`aborted` が立つまで待つ（vitest の試験の timeout が上限）。
  commit「fake-daemon の abort の試験を固定の 30 ms 待ちから出来事待ちにする」。直した後、単独で 2 回 17 passed、`tsc -b` exit 0。

### 2. 範囲の check（exit 1、範囲外 21 path）

範囲の正規表現が許さない path を、ADR 2026-11-21-chat-discussion-mode と objective が変更を求めている。成果をどう直しても通らないので、`plan_issue` で申告した。

- (a) 要求の struct literal（ADR D9「`ChatPostMessageRequest {` 65 か所・`ChatRelayRequest {` 20 か所に `discussion_mode: None` を足す」）。
  欄は ADR D1.1 が両方の要求に置くと決めている。Rust は struct literal に全欄を要るので、欄を足すと次の file を必ず変える:
  `crates/celeris/src/chat_gc_tests.rs`・`crates/celeris/src/daemon/cos_reload_tests.rs`・`crates/celeris/tests/{actor_trace_flow2,actor_trace_flow5,campaign_wiring_e2e,script_generation_e2e}.rs`・
  `crates/task-core/src/store/project_guard_usage_tests.rs`・`crates/task-dispatch/src/cos_context/tests.rs`・
  `crates/task-dispatch/src/dispatcher/tests/{cos_chat_launch,cos_chat_rollover,cos_chat_triage,cos_chat_triage_inbox_thread,cos_digest_trace,cos_launch_wire,cos_live_fix_d4,cron_cos_review}.rs`・
  `crates/celeris-mcp/src/tools/console.rs`（`ChatRelayRequest` の literal。ADR D8 の試験 `discussion_mode_relay_and_mcp_instruct_carry_mode` も MCP の入力の mode を求める）。
- (b) CoS の prompt の hint（objective (4)、ADR D1.2）。worker は `context.cos_chat`（`crates/task-worker/src/protocol/cos_chat.rs` の `CosChatContext`）しか受け取らないので、
  mode を渡すにはこの型に欄が要る。生成物 `docs/protocol/worker-protocol.schema.json` は試験 `committed_schema_matches_generated` が照合する。
  dispatcher 側の導出の呼び出しは `crates/task-dispatch/src/cos_context.rs`（15 行）。
- (c) `crates/task-core/src/discussion_mode.rs`。ADR D1.2 は `task_core::chat` に置くと書くが、`chat` に置くと repo health の `size.responsibility-growth` が落ちる
  （`chat` の本体 8092 行 > 8000、`chat::SqliteStore` の method 115 で増加）。上の「ADR からずれた点」の 1 つ目。
- 範囲の内側だけで済ませる案（要求の型の外で `discussion_mode` を JSON から抜き、schema を `schemars(transform)` で足す等）は、schema と型の正本が 2 つになり（規約 4）、
  ADR D1.1 にも反するので採らなかった。`#[serde(flatten)]` の包む型は内側の `deny_unknown_fields` が効かなくなり、未知の欄の 422 が変わるので採れない。
- 計画への提案: 範囲の正規表現に次を足す。
  `crates/celeris/src/chat_gc_tests\.rs$|crates/celeris/src/daemon/cos_reload_tests\.rs$|crates/celeris/tests/|crates/celeris-mcp/src/tools/console\.rs$|`
  `crates/task-core/src/discussion_mode\.rs$|crates/task-core/src/store/project_guard_usage_tests\.rs$|crates/task-dispatch/src/cos_context(\.rs$|/)|`
  `crates/task-dispatch/src/dispatcher/tests/|crates/task-worker/src/protocol/cos_chat\.rs$|docs/protocol/worker-protocol\.schema\.json$`
  あわせて ADR D1.2 の置き場所を `task_core::discussion_mode` に改める（health の上限のため）。

### Run #2 の検査（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`: exit 0。web は tsc -b 通過・vitest 101 file 705 passed、test-ts-verify 通過、
  nextest は対象 14 crate で 8601 tests run: 8601 passed・31 skipped（481.8 s）、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0（vendor の deno_core の既存の警告 1 件のみ）。
- `cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- 範囲の check: 範囲外 21 path のまま（計画の問題として申告。上の 2）。

## 未解決事項

- 範囲の check は計画の正規表現が直るまで通らない（Run #2 の 2）。
- MCP の入力の `instructed_by`（run の入力の MCP の発言は今も引用できる）は既存の挙動のまま。mode は「させない」向きにだけ効く（ADR D7）。人の承認に数えるかは子 4 の scope の決定に残る。

## 提案

- ADR D1.2 の置き場所の記述（`task_core::chat`）を `task_core::discussion_mode` に改める（close の段か次の ADR 改訂で）。
