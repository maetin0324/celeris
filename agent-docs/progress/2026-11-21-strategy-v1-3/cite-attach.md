---
title: 戦略層 v1 子 3 cite-attach — 案と要約の添付の検査、返事の citations、GET と SSE の失効の表示
tasks: [01M7XQ9QE8STND208NXG90F743]
status: running
updated: 2026-11-22
---
# cite-attach（WorkUnit。子 3 の段 cite）

設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D3・D6・D8.1。基準は `$CELERIS_WU_BASE`。
Run #1（2026-11-22）は実装と試験まで進め、repo health の `size.responsibility-growth` で落ちたため区切った（yield）。

## したこと（Run #1）

- `task_core::chat_citation`（葉）: 保存形 `ChatFileCitation {path, citation}`、wire `ChatCitationView {path, citation, state}`、
  引いた message の今の事実を読む `cited_message_conn`（`discussion_citation::cited_message` から本体を移し、そちらは委譲だけ）、
  状態を導く `citation_views`・`citation_views_of_metadata`。依存は葉の `chat_revision_wire` だけ（循環を足さない）。
- `task_core::chat`: `ChatMessage.citations`（空なら出さない）を `message_from_raw` で導く（GET messages）。
  `chat_events_page`（SSE の replay と GET runs/{r}/events）で message event の引用の状態を今の値に直す。
  `chat_message_attach_workspace_files` に `citations` を足し、同じ transaction で引用を検査し直して `metadata_json.citations` に 1 回だけ書く。
- `task_core::discussion_citation`: `chat_workspace_file_sha256s`（同じ thread の同じ workspace path の添付の sha256。版の衝突の判定）。
- task-dispatch `dispatcher/cos_chat/discussion_files.rs`（新規）: 案は mode（discuss は `discussion_mode_forbidden`）、D3 の固定値、
  各 `chat` 参照の `verify_citation`、子 2 の `strategy::validate_document`（intent は記録、`key_sha256s` に同じ path の既存の sha256）。
  要約は `check_discussion_summary`。`workspace_files::attach` から呼び、通らない file は添付せず `chat_run_status` に `<code>: <path> は添付しなかった` を出す。
- task-worker `cos_chat.rs`: discuss・propose の run の prompt に要約・案の file の書き方と、入力ごとの引用の値を出す。
- `docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}` を再生成、`docs/api/v1/api.md` の GET messages の行に citations を書いた。

## 試験（提出時の HEAD で実行）

- `cargo test -p task-dispatch --lib -- discussion_files` → 7 passed（`chat_discussion_summary_attached_with_citations`・
  `chat_discussion_summary_edit_makes_get_and_sse_edited`・`chat_discussion_summary_other_project_citation_not_attached`・
  `discussion_mode_propose_attaches_valid_strategy_draft`・`discussion_mode_propose_refuses_approved_state_draft`・
  `discussion_mode_discuss_refuses_strategy_draft`・`discussion_mode_propose_same_name_other_sha_refused`）。
- `cargo test -p task-worker --lib -- cos_chat::tests::chat_discussion_summary_prompt cos_chat::tests::discussion_mode_prompt` → 2 passed。
- 変更前に落ちること: `workspace_files.rs` だけを実装前の版に戻して同じ 7 件を走らせ、7 件とも FAILED（例: approved の案と discuss の run の案が添付された、
  `citations` が空）。出力は run の成果物 `negative-before.txt`。確かめた後に元へ戻した。

## 未解決（次の run がする）

- `bash scripts/dev/test-affected.sh` は exit 1。repo health の `size.responsibility-growth` が 2 件 fail:
  - `crates/task-core/src/chat` の本体が 8038 行（閾値 8000。WU の base では約 7999）。この WU の chat への追加（約 +39 行）を葉の module に移し、
    chat の増分を 0 にする。
  - `crates/task-dispatch/src/dispatcher` が 32296（merge-base）→ 32541。`discussion_files.rs` を dispatcher の外（例 `crates/task-dispatch/src/discussion_files.rs`）に移し、
    `workspace_files::attach` の差分を数行にする。
- clippy・fmt の検査は未実行（fmt は `cargo fmt --all` を当てた）。
- CoS の prompt の引用の値は revision を 1 と書く。`CosChatInput` に revision が無いため、入力の前に編集された発言の引用は拒否される（fail closed）。
- chat の案の v2 以降（`supersedes` の chat の添付の形、ADR D3）は未実装。今は v2 以降の chat の案は `strategy_invalid` で拒否される。
  chat の案の `task`・`knowledge`・`artifact`・`commit` の参照は解決しないので `strategy_source_missing` で拒否される（fail closed）。
