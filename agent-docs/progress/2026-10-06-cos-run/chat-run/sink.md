---
task: 01M47J2YNMZ14NQ4AXTNA355AA
wu: sink
status: done
completed: 2026-10-06
---
# CoS chat run の進行を chat_events に写す ChatRunSink

`crates/task-dispatch/src/dispatcher/cos_chat/sink.rs` に `ChatRunSink`（`EventSink` 実装）と終端の写像 `chat_finish_for` を置いた（ADR 2026-10-05 D2/D3）。LLM 呼び出しは無い。要約は worker（adapter）の `summary` だけを使う。

- text → `chat_run_append_text`。offset は store が数える追記前の UTF-8 byte 長。本文は `detail`（無ければ `msg`）
- tool_use / tool_result → `chat_run_tool`。progress に call id が無いので sink が `call-<n>` を振る。結果は同じ道具名の最も古い未完の call を閉じ、名前が無ければ最も古い未完の call を閉じる。終端の時点で未完の call は failed にする
- tool の summary/detail は既存の redact（`task_core::browser_live::persistable` の console 秘密判定）を行ごとに当て、既知の秘密の値（run credential の token。`ChatRunSink::new` の `secrets`）は先に `[redacted]` に置き換える。detail の 4 KiB 上限と truncated は store（`chat_run_tool`）が付ける
- thinking → status phase=thinking。`summary` だけで `detail`（思考本文）は流さない。status・構造の無い progress → phase=working
- heartbeat・artifact は events を増やさない
- `session_established`・`session_resume_failed`・context 圧縮・context 枯渇（`BudgetExhausted{Context}`）は `ChatSessionSignals` に残し、`signals()` で読む（launch・rollover が使う）
- `chat_finish_for`: Done/Question → completed（全文を最終値。空なら途中の本文のまま）、Error/adapter 失敗/Waiting → failed、Yielded/BudgetExhausted → interrupted。呼び出し側の意図 `ChatStopIntent::Stop` → stopped、`Interrupt` → interrupted
- `finish(&ChatFinish, &ParsedActions)`: actions があれば実行せず、kind=notice・state=error・reason 付きの card を system message で出してから `chat_run_finish`。終端の後（他者が store 上で終端にした Conflict を見た後も）は何も書かない

## 証拠
- `cargo test -p task-dispatch --lib cos_chat_run_sink_` → 8 passed（一時 SQLite、固定時計、sleep なし）
- `cargo test -p task-dispatch --lib` → 695 passed, 0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo clippy -p task-dispatch --tests -- -D warnings` → exit 0

## 未解決・統合時の注意
- `cos_chat/mod.rs` は attach 葉も新規作成している（`pub mod attachments;`）。統合では両方の行（`pub mod attachments;` と `pub(crate) mod sink;`）を残す。dispatcher.rs の `pub mod cos_chat;` は attach 葉と同じ行なので衝突しない
- sink は dispatcher の配線前なので `#![cfg_attr(not(test), allow(dead_code))]` を付けた。launch 葉が配線したら外す
- workspace 全体の `test-parallel.sh` は壁時計の予算のためこの葉では回していない（integrate / verify で回る）
