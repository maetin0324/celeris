# chat-run / proto: worker protocol に CoS chat run の入力と前置きを足す

---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
status: done
completed: 2026-10-06
---

決定は [ADR 2026-10-06 cos-chat-run-dispatch](../../../adr/2026-10-06-cos-chat-run-dispatch.md)（一時 Task・`context.cos_chat` の有無で判定・credential は env 名だけ）。

## やったこと
- `crates/task-worker/src/protocol/cos_chat.rs`: `CosChatContext`（thread_id・run_id・inputs・summary/summary_through_seq・unsummarized 範囲・attachments manifest・skills・credential_env・api_base_url）、`CosChatDelivery`（image|file、`for_media_type`）、`CosChatHistory::missing_ranges`、`is_cos_chat_run`、`CosChatContext::transient_task`、`COS_RUN_CREDENTIAL_ENV`。
- `RunContext.cos_chat: Option<CosChatContext>`（skip_serializing_if。None の request.json は不変）。
- `crates/task-worker/src/cos_chat.rs`: CoS chat run の前置き。`claude_code::build_prompt`（全アダプタ共通）が `context.cos_chat` で分岐。旧 actions 指示を外し、actions 不使用・checkpoint API・履歴 API・credential の env 参照・添付の delivery 別の扱い・未要約範囲の明示を書く。
- `task-dispatch/src/dispatcher/worker_task.rs`: 既存の RunContext リテラルに `cos_chat: None`（コンパイルのための 1 行）。
- `docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1` で再生成、`worker-protocol.md` の context 表に 1 行。

## 証拠
- `cargo test -p task-worker --lib cos_chat_run_proto` → 6 passed（absent context 不変・判定は context のみ・delivery 区別・未要約範囲・割り込み先頭と actions 禁止・偽 harness で credential が env にだけ届き request.json/stdout/stderr/prompt に値が無い）
- `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated` → 1 passed（以後 UPDATE_SCHEMA 無しでも test-parallel 内で通過）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4081 passed / 0 failed / 13 ignored
- `sh scripts/dev/check-doc-links.sh` → ok、`check-adr-numbers.sh` → ok

## 未解決事項
- credential を子 process の env に実際に置くのは launch WU（アダプタの env 経路。fake は `set_env`、他アダプタは dispatcher の env 合成）。
- `api_base_url` は dispatcher が `[api] listen` から組む想定（launch WU）。
- 添付 manifest の stage・hash 照合は attach WU、`context.session` の thread 単位化は session WU。

## 提案
- 無し

## 再試行 v2（2026-10-06）
- 前回の不合格は範囲 check のみ（worker_task.rs の `cos_chat: None`）。replan で範囲に許可された。成果 626def22 はそのまま。
- 他の RunContext リテラルは `..Default::default()` 等で通るため変更不要（`cargo check --workspace --all-targets` exit 0）。
- `cargo test -p task-worker`: 全 pass（lib 771 passed、cos_chat_run_proto_ 6 件、committed_schema_matches_generated ok）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
