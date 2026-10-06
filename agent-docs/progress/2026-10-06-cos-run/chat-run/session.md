---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
status: done
completed: 2026-10-06
---
# CoS chat の thread 単位 session（node_sessions の照合・retire・session_mode 記録）

ADR 2026-10-05-cos-chat-home D1/D2 の thread session を、ADR-0054 の `node_sessions`（`kind='cos_chat'`、0050 で足した列）の上に作った。migration は足していない。

- store（`crates/task-core/src/chat/session.rs`）: `ChatSessionKey`（thread_id, harness, provider, llm_source, account_id, cwd, model。harness は `adapter` 列）、`ChatSession`、`chat_session_active` / `chat_session_get` / `chat_session_rotate`（現役の retire と新しい行の作成を 1 transaction で行う）/ `chat_session_retire` / `chat_session_touch` / `chat_session_set_id` / `chat_session_set_summary_through`（下げない）。
- run への記録: `chat_run_record_session(run_id, ChatRunSessionMode, session_row_id, reason)` は `chat_runs.session_row_id` に加えて、`resolved_config_json` に wire の `session_mode`（new/resumed/fresh）、詳しい `session_detail`（new/resumed/fresh/fresh_after_refusal）、`session_reason` を書き、run event を出す。GET と SSE の `R.session_mode` は既存の `run_from_raw` がそのまま読む。wire enum は変えていないので、schema の再生成は要らない。対象は live な run のうち同じ thread の行に限る（他は 409・404・Invalid）。
- 判定（`crates/task-dispatch/src/sessions/cos_chat.rs`、純関数）: `decide_cos_chat_session` は New / Resume / Retire(5 種) を返す。5 種は KeyChanged・CacheMissing（cache 無し、または claude の UUID でない・空 id）・ResumeRefused・TokenRollover（`rollover_tokens` 以上）・ContextExhausted。優先順は、key 変更 → 拒否 → cache → 枯渇 → token。`run_mode()` で ResumeRefused を FreshAfterRefusal、他の retire を Fresh に写す。UUID の検査は既存の `session_id_is_valid_for_adapter` を使う。account cache をコピーしない条件は、呼び出し側が `cache_present`（この run の account の config dir にあるか）を渡すことで守る。

## 証拠
- `cargo test -p task-core --lib cos_chat_run_session` → 5 passed（rotate の 1 thread 1 現役、touch/set_id/watermark、session_mode の記録と GET・run event、他 thread の行・終わった run の拒否、conversation/continuation の行に干渉しない）
- `cargo test -p task-dispatch --lib cos_chat_run_session` → 6 passed（初回 New・一致で Resume、key 7 要素それぞれの変更、cache 消失と不正 id、拒否→fresh_after_refusal、token 超過と枯渇 event、理由の優先順）
- `cargo clippy --workspace -- -D warnings`（`--all-targets` でも）→ exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4086 passed / 0 failed / 13 ignored（既存の node_sessions・continuation の試験を含む）

## 未解決事項
- 照合 key・cache の有無・前回の拒否と枯渇は、launch / rollover 葉の dispatcher が集めて渡す（この葉は store と判定だけ）。fresh 再試行を 1 回に限ることは rollover 葉で扱う。

## 提案
- なし
