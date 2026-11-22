---
title: 戦略層 v1 子 3 cite-attach — 案と要約の添付の検査、返事の citations、GET と SSE の失効の表示
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
---
# cite-attach（WorkUnit。子 3 の段 cite）

設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D3・D6・D8.1。基準は `$CELERIS_WU_BASE`。
Run #1（2026-11-22）は実装と試験まで進め、repo health の `size.responsibility-growth` で落ちたため区切った（yield）。
Run #2（2026-11-22）は置き場所を直して size の 2 件を解き、検査を提出時の HEAD で回した。

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

## したこと（Run #2）: repo health の size の 2 件を解く

- 添付の関数 `chat_message_attach_workspace_files`（引用の検査し直しと `metadata_json.citations` の書き込みを含む）を
  `chat/run_store.rs` から新しい兄弟 module `task_core::chat_workspace_files` に移した。版の衝突の入力
  `chat_workspace_file_sha256s` も `discussion_citation` から同じ module に移した。依存は chat と葉の chat_citation への一方向。
- `ChatMessage.citations`（Vec）は `ChatEventData` を clippy の `large_enum_variant` に掛けたので、edit-retract の
  `edit_state` と同じ形（`#[serde(flatten)] cited: Box<ChatMessageCitations>`）にした。wire の形（message の `citations`）と
  生成物（`docs/api/v1`・`web/api/generated`）は Run #1 と同じ（再生成して差分 0）。
- SSE の replay の状態の更新を `chat_citation::refresh_citation_states` にまとめた。`discussion_citation::cited_message`
  （`cited_message_conn` に委譲するだけの重複）を消した。
- 案・要約の検査を `dispatcher/cos_chat/discussion_files.rs` から `cos_context/discussion_files.rs` に移し、
  添付の繰り返しの中の処理を `AttachGate { upload(), finish() }` にまとめた。`workspace_files::attach` の差分は
  gate の生成・`upload`・`finish` の 3 か所だけ（本体の行は base と同じ数）。置き場所は WU の範囲 check が許す
  dispatcher の外の path（`crates/task-dispatch/src/cos_context`）に合わせた。
- 本体の行（repo health の数え方を同じ関数で当てた）: `crates/task-core/src/chat` は WU の base より 48 行減、
  `crates/task-dispatch/src/dispatcher` は 1 行減。

## 検査（提出時の HEAD で実行）

- `python3 -I scripts/dev/repo-health/repo_health.py --repo . --base <main との merge-base> --head HEAD --policy-version 1
  --rules size.responsibility-growth,deps.cycle,deps.violation` → verdict=pass、new=0、worsened=0。
- 範囲 check（WU の check の 5 番目と同じ正規表現）→ 範囲外の path なし。
- `cargo test -p task-dispatch --lib -- discussion_files` → 7 passed。
- `cargo test -p task-core --lib -- chat::store_tests discussion_citation chat_citation` → 41 passed（module 移動の直前の HEAD）。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib -- schema::` → 2 passed、`node web/scripts/gen-types.mjs` → 生成物の差分 0。
- `bash scripts/dev/test-affected.sh`（`cos_context` へ移す直前の HEAD）→ exit 0。nextest 8637 passed・31 skipped、
  web の型検査と unit 101 files・705 passed、ts-verify 20 passed、repo health verdict=pass。
  optional の `dup.state` の worsened 2 件（chat_threads・chat_messages の書き手が 3 dir）は WU の base でも同じ 2 件。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（同じ HEAD）。
- 提出時の HEAD での test-affected・clippy・fmt の再実行の結果は、WU の報告（result.json）に書く。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）

- production の Rust: +705 −126（discussion_files の検査と gate、引用の型と導出、添付の module の移動を含む）。
- 試験: +530 −4。その他（生成の schema・型、api.md、進捗）: +224 −1。
- 純増の理由: 子 2 の検査器と summary-cite の関数を run の終わりの添付につなぐ新しい経路（検査・引用の書き込み・状態の導出）で、
  消せる旧い経路は無い（既存の添付の経路をそのまま延ばした）。

## 未解決

- CoS の prompt の引用の値は revision を 1 と書く。`CosChatInput` に revision が無いため、入力の前に編集された発言の引用は拒否される（fail closed）。
- chat の案の v2 以降（`supersedes` の chat の添付の形、ADR D3）は未実装。今は v2 以降の chat の案は `strategy_invalid` で拒否される。
  chat の案の `task`・`knowledge`・`artifact`・`commit` の参照は解決しないので `strategy_source_missing` で拒否される（fail closed）。
- `AttachGate::finish` の 409（検査と添付の間に引いた発言が編集された）の分岐は試験が無い（時計の差し替えでは作れず、検査と添付の間に
  編集を挟む試験専用の hook が要る）。

## 提案

- `cos_context` は「CoS の run が読む文脈」の module で、出力の file の検査は意味が少し違う。範囲 check を
  `crates/task-dispatch/src/discussion_files.rs` まで広げられるなら、crate 直下の module に移すのがよい（integrate-cite か close で判断）。
