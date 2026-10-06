# harness-adapters / codex: CoS 継続（exec resume）・全道具・画像入力

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
status: done
completed: 2026-10-06
---

## 変更（`crates/task-worker/src/codex.rs`。CoS 判定は `context.cos_chat` の有無だけ）

- 継続: 初回は `codex exec`。`thread.started` の id を `sink.session_established` で確定（既存の経路）。以後 dispatcher が `context.session.resume=true` を渡すと `codex exec resume <id>`（`resume_mode`/`resume_bypass` の既存規則のまま）。
- 明示 fresh: CoS run で resume を保証できない設定では resume せず fresh で起動し、`capability_reason(Continuation)` に理由を足した status を出す。対象は (a) `resume_mode = experimental`（`-c experimental_resume` は保証にならない）と (b) `exec resume` に翻訳できない `extra_args`（ADR-0095 D-b の bypass は使わない）。prompt は `cos_chat::build_prompt` なので要約と未要約の DB 履歴が入る。
- 道具: CoS run は `sandbox_mode="workspace-write"`（従来の Secretary 用 read-only 固定を CoS run には適用しない）。fresh では `--add-dir`（artifacts・git 管理領域・CARGO_TARGET_DIR）も従来どおり付く。禁止フラグは使わない。
- 写像: `agent_message`→Text（CoS は改行を保った全文を detail に入れる。text_delta 用）、`command_execution`・`mcp_tool_call`→ToolUse/ToolResult、識別不能な item→Status（既存 `item_progress`）。
- 返事: CoS run は一時 Task に `conversation` が無いので、`result.json` が無いとき最後の `agent_message` を `Done.summary` にする（`result.actions` 回収の経路は CoS では使わない）。
- 画像: `image_delivery`（caps 層。prompt の `actual=` と同じ判定）で native なら `--image <path>` を `--json` の前に置く（fresh と resume の両方。`--image` は複数値を取るので末尾の `-` を食わせない）。`,` を含む path は codex が分割するので run dir に写してから渡す。path+tool・unsupported は `--image` を付けず、`attachment <id>: <reason>` の status を出す。

非 CoS（`cos_chat` 無し）の挙動は変えていない。Secretary の read-only・直接の返事の扱い・resume 規則は既存試験と `cos_chat_harness_codex_non_cos_secretary_stays_read_only` で確認した。

## 確認

- `cargo test -p task-worker --lib cos_chat_harness_codex`: 9 passed（初回の thread id 確定、exec resume、experimental と翻訳不能 extra_args の明示 fresh＋DB 履歴、message/command/MCP/status の写像、画像 native（fresh・resume）、`,` 入り path の写し、unsupported・path+tool、非 CoS の read-only）。
- `cargo test -p task-worker --lib codex`: 91 passed（既存の codex 試験を含む）。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `bash scripts/dev/test-parallel.sh`: 結果は下に追記。

## 未解決事項

- 実 CLI での `codex exec resume --image` の受理は未確認（`--help` には `-i/--image` がある。Phase 68c のとおり、本番の usage 行は `--help` より狭いことがある）。拒否されたら `--image` は fresh のみに絞り、resume 時は path+tool に落とす必要がある。
- `harness_capabilities` は dispatcher が `None`（未確認）で渡しているので、今のままでは画像は unsupported になる。確認済みの能力を誰が埋めるかは config / e2e の WorkUnit の範囲。

## 提案

- resume 時の `--image` 受理を実機で確かめる手順（人）: `codex exec resume --help` の確認に加え、捨てて良い thread で `codex exec resume <id> --image x.png --json --skip-git-repo-check -` を 1 回流す。
