---
title: worker が内部で subagent や別の LLM CLI を起動できないようにする（全 adapter の既定禁止・前置きの規則・検出と記録）
tasks: [01M49WJP27NNVDE5E3FCY82DDP]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# worker が内部で subagent や別の LLM CLI を起動できないようにする

設計は [ADR 2026-10-07-worker-no-subagents-no-llm-cli](../adr/2026-10-07-worker-no-subagents-no-llm-cli.md)。
人の指示（2026-10-07）: run 01M49KWW6J255FA99BJSFHS05E（claude-code）が計画のレビューを `Agent` 道具の subagent に任せ、
人の確認になるはずの段が worker の内部で済まされ、使用量が run・routing・quota の記録の外に出た。応急処置の
`[adapters.claude_code] extra_args = ["--disallowedTools=Agent,Task"]` をコードの既定に置き換える。

## やったこと（1 run、2026-10-07）

- **一次情報の確認**（この host の実機）: Claude Code 2.1.287（`--disallowedTools`、`Agent`/`Workflow`、`--forward-subagent-text`）、
  Codex CLI 0.160.1（`codex features list` → `multi_agent stable true`、`-c features.multi_agent=false` で false、未知 feature は exit 0）、
  opencode 1.18.35（binary 内の `task` 道具 "Launch a new agent to handle complex, multistep tasks autonomously." と
  `subagent_type`、`OPENCODE_CONFIG_CONTENT` を local 層として読む、SDK 1.18.31 `Config.tools: {[id]: boolean}`）。ADR §1.1 の表。
- **D1 claude-code**: `--disallowedTools Agent,Task,Workflow` を `extra_args` の**後ろ**に付ける（`crates/task-worker/src/claude_code.rs`）。
- **D2 codex**: `-c features.multi_agent=false` と `-c features.multi_agent_v2=false` を fresh / `exec resume` の両方で
  `extra_args` の後ろ・`-` の直前に付ける（`crates/task-worker/src/codex.rs`）。
- **D3 acp（opencode）**: `OPENCODE_CONFIG_CONTENT` の JSON に `tools.task=false` を重ねる（運用側の env と
  `routing_context_transport::configure_acp` の provider header overlay と同居。`crates/task-worker/src/acp.rs`）。
- **D4 前置き**: `preamble::tool_launch_policy_note`（常に出る 3 節目。claude-code / codex / acp / paperqa に届く）。
- **D5 検出**: `crates/task-worker/src/tool_policy.rs`（新規。`inspect_tool_use` / `inspect_command`、`SUBAGENT_TOOLS` /
  `LLM_CLIS` / `LLM_API_HOSTS`）。3 adapter の `tool_use` の写像から呼び、`EventSink::policy_violation` と
  `WorkerProgress`（`status`・`error = true`・`policy: …`）に出す。dispatcher の `StoreSink` / `ReviewerSink` が
  `Event::WorkerPolicyViolation { run_id, kind, tool, matched, command }` を追記（`EVENT_TYPES` 68 種）。
- **D6 reviewer**: `ReviewRequest.policy_violations`（`review_spawn::review_policy_violations` が対象 run の event から集める）と
  review prompt の節 "Tool policy violations recorded by celeris (authoritative)"。
- **D7 設定**: `[adapters.claude_code|codex|acp] subagents = "deny"（既定）| "allow_cos" | "allow"`（`SubagentPolicy`。
  `allow_cos` は `conversation_addressee == Secretary` の run だけ）。
- schema 再生成: `docs/api/v1/{api-v1,event}.schema.json`、`docs/protocol/worker-protocol.schema.json`、
  `web/api/generated/{schema.json,types.ts}`、`gui/app/celeris/types.ts`。`web/api/realtime/{event-kinds,invalidation-map}.ts` に
  `worker_policy_violation`。`docs/api/v1/gui-api.md` §3.6 と `docs/architecture-map.md` に追記。

## 証拠（コマンドと結果）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. 既定禁止（claude-code argv・codex argv・acp env） | `cargo test -p task-worker -- subagent multi_agent opencode_config_content task_tool_override command_line_has_exec phase_68b f5_fix4` | 合格（`subagent_tools_are_disallowed_by_default_after_any_extra_args`、`only_an_explicit_subagents_setting_lifts_the_deny`、`exec_resume_also_disables_multi_agent_by_default`、`only_an_explicit_subagents_setting_keeps_multi_agent_enabled`、`opencode_config_content_disables_the_task_tool_by_default`、`the_task_tool_override_composes_with_the_routing_context_overlay` ほか） |
| 0. codex / ACP の一次情報 | `codex features list`、`codex features list -c features.multi_agent=false`、`strings ~/.opencode/bin/opencode` | ADR §1.1 に記録 |
| 1. 前置きの規則 | `cargo test -p task-worker -- preamble tool_launch` | 合格（`the_tool_launch_policy_rule_is_always_in_the_preamble`、byte 一致の既存試験は 3 節の連結に更新） |
| 1. 検出と event | `cargo test -p task-worker -- tool_policy policy`、`cargo test -p task-dispatch -- review_policy_violations`、`cargo test -p task-api -- worker_policy_violation` | 合格（`tool_uses_that_launch_subagents_or_other_llms_are_reported_to_the_sink`、`command_executions_that_launch_other_llm_clis_are_reported`、`acp_tool_calls_that_spawn_subagents_or_launch_llm_clis_are_reported`、`review_policy_violations_collects_only_the_subject_runs_detections`、`the_review_prompt_lists_recorded_tool_policy_violations`） |
| 2. CoS の例外と config | `cargo test -p celeris -- adapter_subagents` | 合格（`adapter_subagents_setting_resolves_deterministically_and_defaults_to_deny`） |
| 2. 全体 | `bash scripts/dev/test-parallel.sh` | 下の「検査結果」 |
| 2. clippy | `cargo clippy --workspace -- -D warnings` | 下の「検査結果」 |
| web | `pnpm -C web typecheck` / `lint` / `test` | exit 0 / exit 0（既存 warning 4）/ 70 files 468 tests pass |
| gui | `pnpm typecheck` / `test`（cwd gui） | exit 0 / 96 files 1341 tests pass。`pnpm lint` は main 由来の 5 件（`app/root.tsx`・`app/routes/notifications.tsx`・`scripts/check-resume-recovery.mjs`・`test/unit/notifications*.test.ts` の format / useTemplate）で exit 1。本 task が触った `gui/app/celeris/types.ts`（生成物）は指摘なし |

## 検査結果（統合 HEAD、2026-10-07、commit 前の作業ツリー）

| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY`: nextest 0.9.146、binaries 140、passed 4207、failed 0、ignored 14（nextest 112 s、doctest 11 s） |
| `cargo clippy --workspace -- -D warnings` | exit 0、warning 0 |
| `cargo fmt --all -- --check` | exit 0 |
| 受け入れ条件の試験（`cargo nextest run -p task-worker -p task-dispatch -p celeris -p task-api -E 'test(subagent) \| test(multi_agent) \| test(opencode_config_content) \| test(task_tool_override) \| test(tool_launch) \| test(policy) \| test(worker_policy_violation)'`） | 61 passed、0 failed |
| `sh scripts/dev/progress-index.sh --check` / `check-adr-numbers.sh`（150 files） / `check-doc-layout.sh scripts/dev/docs-layout.tsv` / `check-doc-links.sh` / `python3 scripts/dev/check-architecture-map.py`（267 path） | すべて ok |
| `pnpm typecheck` / `pnpm test`（cwd web） | exit 0 / 70 files 468 tests pass |
| `pnpm typecheck`（cwd gui） | exit 0 |

1 回目の `test-parallel.sh`（実装直後）は 7 件落ちた（task-api の EVENT_TYPES 固定 2 件、codex argv の既存試験 3 件、
codex worktree cwd 1 件、worker-protocol schema の固定 1 件）。いずれも新 event / 新 argv に固定値を合わせる修正で、
2 回目以降は 0 fail。

## 未解決事項・人への問い

- **D7（人の決定）**: CoS に subagent を許すか。選択肢は ADR D7 の表（A: CoS も禁止＝既定・推奨、B: `subagents = "allow_cos"`、
  C: `"allow"`）。A なら config は何も変えない。
- 本番: この release の後、運用セッションが `[adapters.claude_code] extra_args` の `--disallowedTools=Agent,Task` を外す
  （重なっていても害は無い）。本 task では本番 config に触れていない。
- CoS chat（別 branch の ADR 2026-10-05-cos-chat-home）の run の印は `Secretary` 以外かもしれない。取り込み時に `SubagentPolicy::allows`
  の判定にその印を足す（統合する側の task）。
- opencode の `tools.task=false` は SDK の型と binary の config 読み込み経路からの推定で、**実機の opencode で `task` 道具が
  消えること**は確認していない（worker は外部ネットワークに出ない・ACP 実機を sandbox で起こせない）。人の確認手順:
  `OPENCODE_CONFIG_CONTENT='{"tools":{"task":false}}' ~/.opencode/bin/opencode run 'list your tools'` で `task` が無いこと。
- 検出は警告で止めない。誤検出（`claude --version` の確認、API host を含む curl 以外の command）はあり得る。reviewer が判断する。
- gui `pnpm lint` の 5 件は main（95769271）由来。別 task で直す。

## 提案

- PATH の wrapper（D8 で見送り）: 実機で D1–D5 をすり抜ける起動が見えたら、`PATH` 先頭に拒否 script を置く案を再検討する。
- web/gui に `worker_policy_violation` の専用表示（run の見出しに警告バッジ）を足す。今は `worker_progress` の error 行として見える。
