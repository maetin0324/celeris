---
title: CoS chat run は result.json 不在でも返事の本文で Done にする
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-07
---
# CoS チャットホーム — result-fix WorkUnit

live-check の不具合 1（claude-code の CoS chat run が result.json 不在で必ず failed）を直した。CoS chat の前置き（`cos_chat::build_prompt`）は result.json を求めず返事を本文で流すので、adapter 側も `context.cos_chat` の run では result.json を必須にしない。

## 変更

- `crates/task-worker/src/claude_code.rs`: `terminal_from_result` が `RESULT_JSON_MISSING_MARKER` を返したとき、`context.cos_chat` かつ `is_error=false`・`subtype=success` なら `Terminal::Done{summary}` にする。summary は `result` の本文、空なら流した assistant の `text` の連結（`push_assistant_text`）。result.json があれば従来どおりそれが勝つ。失敗の `result` は failed のまま。
- `crates/task-worker/src/codex.rs`: 変更なし。`turn.completed` で result.json が無ければ、cos_chat の run は最後の `agent_message` で既に Done になっていた（前の偽 codex も result.json を書いていない）。試験で固定した。
- `crates/task-worker/src/acp.rs`: `RawOutcome::Success` に `cos_chat_reply`（cos_chat の run の `agent_message_chunk` の連結）を足した。`stopReason=end_turn` で result.json が無ければ Done。他の stop reason は従来どおり。
- `crates/task-worker/src/pi.rs`: `PiStream.last_reply`（最後の assistant message の text）。cos_chat で result.json が無ければ Done。
- cos_chat でない run の挙動は変えていない（既存試験は不変で全部通る）。`dispatcher/cos_chat/launch.rs` は触っていない。

## 試験

- task-worker: `cos_chat_harness_claude_done_without_result_json`（result 本文・流した本文への fallback・result.json が勝つ）、`cos_chat_harness_claude_error_without_result_json_stays_failed`、`cos_chat_harness_claude_non_chat_run_still_requires_result_json`、`cos_chat_harness_codex_done_without_result_json`、`cos_chat_harness_codex_failed_turn_without_result_json_stays_failed`、`cos_chat_harness_acp_done_without_result_json`（refusal は failed）、`cos_chat_harness_pi_done_without_result_json`（非 cos_chat は failed）。
- task-dispatch: `cos_chat_harness_e2e_claude_without_result_json_completes`（偽 claude は result.json を書かない。chat run は completed、reason なし、出力 message の本文が「了解しました」）。

## 検証

| コマンド | 結果 |
|---|---|
| `cargo nextest run -p task-worker -E 'test(cos_chat_harness_) \| test(pi_adapter)'` | 47 passed |
| `cargo nextest run -p task-dispatch -E 'test(cos_chat_harness_)'` | 4 passed |
| `bash scripts/dev/test-parallel.sh` | exit 0、passed 4595 / failed 0 / ignored 14 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all` | 差分は適用済み |

## 未解決事項

- 実機（LLM を伴う）での確認は、この WU ではしていない。patch なしの tree での再確認は葉 live-check2 が行う。
- live-check の不具合 2（CoS run の celerisctl が既定で本番の設定を読む）は並行の葉 api-env、不具合 3（KB に cos skill が無いと動かない）の ops 文書は葉 ops-docs2 が扱う。

## 提案

- なし。
