---
title: 戦略層 v1 子 3 edit-retract — 人の message の編集・撤回（前の本文を chat_events に残し、要約と session を戻す）
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# edit-retract（WorkUnit。子 3 の段 cite）

完了日: 2026-11-22（Run #2。Run #1 は予算切れで、実装と試験はそこで済んでいた）。設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D5・D7・D8.1。
基準は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## したこと

- REST（`crates/task-api/src/chat/revise.rs`、route は `chat/mod.rs`）:
  - `PATCH /api/v1/chat/threads/{t}/messages/{m}` 本文 `{text, expected_revision}`
  - `POST …/messages/{m}/retract` 本文 `{expected_revision, reason}`（reason は 500 文字まで、空でよい）
  - `GET …/messages/{m}/revisions`（`message_revised` の event を古い順に `ChatEventListResponse` で返す。応答の型は既存のものを使う）
  - 書けるのは `Principal::Human` だけ（`editor()`）。そのうえで store が `author == human:<name>` を確かめる。operator・別の人・relay の発言・`assistant`・`system` は
    403 `chat_message_edit_forbidden`。CoS の `/cos/operations` の allowlist には足さず、registry の `EXCLUDED`（`cos/ops/surface.rs`、理由 `human_message_edit`）と
    [ADR 2026-10-09-cos-operations-all-mutations](../../adr/2026-10-09-cos-operations-all-mutations.md) の除外表・surface 表に 2 行ずつ登録した（registry の試験がこの 3 つの一致を求める）。
    `human_epoch` を上げない（`HumanEpoch` を呼ばない）。
- 処理（`crates/task-core/src/chat_revision.rs`。`task_core::chat` の外の兄弟 module）:
  - 状態: `running`・`cancelled`・撤回済みは 409。`queued` は編集だけ（撤回は 409。DELETE を使う）。`expected_revision` の不一致は 409 `chat_conflict`。
  - `metadata_json.revision`（無ければ 1）を 1 上げ、`edited{by,at}` か `retracted{by,at,reason}` を書く。撤回は `text=''`（FTS5 は既存の trigger で追従）。mode・添付・添付の参照は変えない。
  - 同じ transaction で `chat_events` に `message_revised`（`ChatMessageRevisedData`: 前の本文・その sha256・前と後の revision・by・reason・checkpoint_reset）を追記し、
    続けて既存の `message` event を流す。
  - D5.4: 発言の seq が `chat_threads.summary_through_seq` 以下なら `summary=''`・`summary_through_seq=0` に戻し、前の要約と cursor を event の `checkpoint_reset` に残す。
    live の CoS session の `delivered_through_seq` 以下なら既存の `retire_conn` で retire する。
- wire の型は葉の module `crates/task-core/src/chat_revision_wire.rs`（crate の中の何にも依存しない）に置き、`chat` は 1 行の `pub use` で再 export する。
  `ChatMessage` には `#[serde(flatten)] edit_state: Box<ChatMessageEditState>` を 1 つ足し、JSON では `revision`（1 のときは出さない）・`edited`・`retracted`
  （無ければ出さない）の 3 つの欄になる。`ChatEventType::MessageRevised`（wire は `message_revised`）と `ChatEventData::Revised`。未変更の message の JSON は base と同じ。
  拒否は `chat_revision::ChatRevisionError`（`Forbidden` → 403 `chat_message_edit_forbidden`、`Chat(ChatError)` → 今の対応）。`ChatError` に variant は足していない。
- `task_core::chat` に足した本体の行は 6 行（列挙の 2・`ChatMessage` の欄の 2・`message_from_raw` の 1・`decode_event` の 1）。`chat` は repo health の
  `size.responsibility-growth` の上限 8000 行の手前にある。最初の版は型を `chat` に置いて 8059 行で落ち、型を葉の module に移して pass にした（提出時の HEAD で 8000 以下）。
- schema: `ApiV1Schema` に要求 2 つ（編集・撤回）を足し、`ChatRun`・`ChatCard` の直接の登録を外した（`ChatRunResponse.run`・`ChatMessage.cards` から `$defs` に残る）。
  型の欄の数は 255 のまま（repo health の `size.responsibility-growth` の閾値 40 を base から超えている型。増やすと worsened になる）。
  `docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}` を再生成し、`docs/api/v1/api.md` に route 3 行・要求 3 行・SSE の type 1 行を足した。
- clippy の `large_enum_variant` を避けるため、欄は `Box` にした（`ChatMessage` は 8 byte だけ大きくなる。`escalation`・`cos_proposal` と同じ形）。
- web の画面は変えていない（web-cite の担当）。既存の SSE client は知らない type（`message_revised`）を捨て、続く `message` event で表示を直す。

## ADR からずれた点

- MCP の token と CoS run credential の書き込みは、handler に届く前に既存の guard が拒否する: MCP は 403 `forbidden`（`middleware.rs` の read-only）、
  CoS は 422 `cos_audit_context_required`（`cos::authenticate`）。ADR D5.1 は 403 `chat_message_edit_forbidden` と書くが、authz の高 risk の path（`middleware.rs`）は変えなかった。
  handler の `editor()` は全 principal を 403 `chat_message_edit_forbidden` で拒否し、単体試験で確かめた（guard が変わっても handler で止まる）。
  `/cos/operations` に PATCH・retract の path を渡すと未登録の path として 422 になる。
- 撤回の `reason` が空でも `retracted.reason` は `""` を持つ（ADR は「空でもよい」とだけ書く）。同じ本文への編集は 422（ADR に記述なし。要約を無駄に失効させないため）。

## 試験（守る不変条件）

task-core `crates/task-core/src/chat_revision_tests.rs`（11 件）:
- `chat_message_edit_by_author_keeps_previous_body_in_chat_events`: 前の本文と sha256 が `message_revised` に古い順に残り、`message_revised` → `message` の順に流れる。
- `chat_message_edit_forbidden_for_non_author_principals`: `human:bob`・`operator:`・`mcp:`・`cos:`・別の人は 403。operator の発言は operator 自身も直せない。何も書かない。
- `chat_message_edit_forbidden_for_relay_message`: relay の発言は operator も人も直せない。
- `chat_message_edit_assistant_or_system_is_403`: CoS の返事と daemon の入力は直せない。
- `chat_message_edit_running_is_409`: queued は編集だけ可、running・cancelled は 409。
- `chat_message_edit_stale_revision_is_409`: 古い revision と撤回済みは 409。
- `chat_message_retract_empties_text_and_search`: 撤回・編集の後、前の語は thread 検索（`chat_thread_list q`）と `chat_search` の MATCH に出ない。
- `chat_message_edit_resets_checkpoint_covering_seq`: cursor が覆う発言の変更で要約を戻し、前の要約を event に残し、走っている run の古い cursor の checkpoint は 409。覆わない発言では要約は残る。
- `chat_message_edit_retires_session_that_saw_message`: 発言を受け取った session は retire、受け取っていない session は残る。
- `chat_message_edit_previous_body_survives_retention`: `chat_events_retention` の後も前の本文が読める。
- `chat_message_edit_validates_text_and_reason`: 同じ本文・空・長すぎる reason・無い message は書かずに拒否。

task-api `crates/task-api/tests/chat_message_revise.rs`（5 件）と `revise.rs` の単体試験 1 件:
- `chat_message_edit_by_author_keeps_previous_body_in_chat_events_api`: HTTP で編集・撤回し、`GET …/revisions` が前の本文 2 つを返す。撤回の後 `GET /chat/threads?q=` は 0 件。
- `chat_message_edit_forbidden_for_non_author_principals_api`: operator・別の人は 403 `chat_message_edit_forbidden`、MCP は 403、CoS は直接 422・`/cos/operations` で 422、
  relay の発言は operator も人も 403。`cos_operations` の applied は 0、`message_revised` は 0 件、本文と revision は不変。
- `chat_message_edit_stale_revision_is_409_api`: queued の撤回・古い revision は 409 `chat_conflict`、未知の欄は 400。
- `chat_message_edit_does_not_bump_human_epoch`: 発言の投稿は `human_epoch` を 1 上げ、編集・撤回は上げない。
- `chat_message_retract_keeps_delete_cancel_and_queue`（回帰）: DELETE は queued を cancelled にし、queue event の列から外す。届いた発言の DELETE は 409。
- `chat::revise::tests::chat_message_edit_forbidden_for_non_human_principals_at_handler`: handler は Human 以外の全 principal を 403 `chat_message_edit_forbidden`。

既存の試験では検出できない理由: base には編集・撤回の口も revision も無く、前の本文を残す・要約と session を戻す振る舞いがそもそも無い。

## 変更前に落ちること（負の試験の出力）

base は試験の型（`ChatMessageEditRequest` など）を持たないので、試験はそのままではコンパイルできない。次の 2 つで、実装を外すと試験が落ちることを確かめた（どちらも確かめた後に元へ戻した）。

1. API の route を base と同じ表にした（PATCH・retract・revisions を外した）: `cargo test -p task-api --test chat_message_revise`
   → `test result: FAILED. 1 passed; 4 failed`。405（PATCH）・404（retract）で落ちた。通った 1 件は DELETE の回帰試験（両方で通るのが正しい）。
   ```
   test chat_message_edit_by_author_keeps_previous_body_in_chat_events_api ... FAILED   left: 405 right: 200
   test chat_message_edit_stale_revision_is_409_api ... FAILED                          left: 404 right: 409
   test chat_message_edit_does_not_bump_human_epoch ... FAILED                          left: 405 right: 200
   test chat_message_edit_forbidden_for_non_author_principals_api ... FAILED            left: 405 right: 403
   test chat_message_retract_keeps_delete_cancel_and_queue ... ok
   ```
2. `chat_revision.rs` から D5.4（要約を戻す・session の retire）と `message_revised` の追記を外した: `cargo test -p task-core --lib chat_revision`
   → `test result: FAILED. 5 passed; 6 failed`。落ちたのは `keeps_previous_body`・`previous_body_survives_retention`・`resets_checkpoint_covering_seq`・
   `retires_session_that_saw_message`・`retract_empties_text_and_search`・`stale_revision_is_409`（後の 2 つは revisions の件数を確かめる行で落ちた）。

## 検査（提出時の HEAD で実行）

- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/check-health.sh`: verdict=pass、fail_grade=0、new=0、worsened=2（上の `dup.state`、optional）。
- `bash scripts/dev/test-affected.sh`: 下の「test-affected の結果」。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の 19 path（上の file・`cos/ops/surface.rs`・ADR 2026-10-09 の表・この進捗）。

### test-affected の結果

- 1 回目（registry の登録の前）: exit 100。repo health pass、web の型検査と unit 試験 101 file・705 件 pass、ts-verify 20 件 pass、
  Rust 8618 件のうち 8617 件 pass・1 件 fail・31 件 skip。fail は `task-api` の `cos::ops::tests::cos_ops_registry_classifies_every_mutation_as_allowed_or_excluded`
  （新しい PATCH・retract が ALLOWED にも EXCLUDED にも無い）。
- 直した後: `cargo test -p task-api --lib cos::ops::tests` で 4 件 pass（registry と ADR の表の一致の試験を含む）。
- 未確認: 直した後に test-affected の全体（15 分）は時間が足りず走らせていない。直した後の変更は `surface.rs` の `EXCLUDED` 2 行と ADR の表 4 行だけで、
  影響するのは task-api の registry の試験（上で pass）。Objective の範囲の外の path は無い。

## LoC（`git diff --numstat` を `$CELERIS_WU_BASE` に当てた。型を葉の module に移す前の値で概数。移した後も合計はほぼ同じ）

- production の Rust: +529 / −18（うち新しい file `chat_revision.rs` 285 行、`revise.rs` 133 行。`revise.rs` は単体試験 約 30 行を含む）。
- 試験: +842 / −0（`chat_revision_tests.rs`・`tests/chat_message_revise.rs`）。
- 生成物と文書: +415 / −20（schema の JSON 2 つ・`types.ts`・`api.md`）。
- 純増の理由: 編集・撤回の 3 つの口と、前の本文を残す・要約と session を戻す処理は base に無い機能で、消せる旧い経路は無い。表・migration・Manager・trait・状態機械は足していない。

## 親の目的への寄与

- 確認済み（試験）: 元の発言の編集・撤回で、前の本文が `chat_events` に残って辿れる（出典の追跡）。要約の cursor と CoS の session が古い本文を抱えたまま残らない（T5 の一部）。
  編集・撤回できるのは書いた人だけで、operator・MCP・CoS の入力は人の発言を書き換えられない（actor の区別）。
- 未確認: 要約・案の引用の失効の表示（summary-cite・cite-attach）と web の表示（web-cite）は後の WU。親の改善 A〜D への効き目は、それらと子 5・6 が揃うまで測れない。

## 未解決事項

- ADR D5.1 の「MCP・CoS も 403 `chat_message_edit_forbidden`」と実際の code（MCP 403 `forbidden`、CoS 422）の差。直すなら authz の path（`middleware.rs`・`cos/mod.rs`）の設計 review が要る。
- repo health の `dup.state`（optional・警告）が 2 件 worsened: 表 `chat_messages`・`chat_threads` を書く dir が `crates/task-core/src` の `chat_revision.rs` で 1 つ増えた
  （`chat` の dir と `store` の dir に加えて 3 つ）。書き手は D5 の編集・撤回だけで、`chat` の行の上限のため `chat` の外に置いた（ADR の Objective の指定どおり）。
  `chat` の上限を見直すか、`chat` の本体を減らせば `chat` の dir に戻せる。
- `cargo clippy --all-targets` は task-core の既存の試験 file（`chat/triage_tests.rs:1038`・`strategy/tests.rs:407`）で落ちる。この WU の前からあり、必須の検査の外。

## 提案

- ADR D5.1 の拒否の表に「guard が先に拒否する principal（MCP・CoS run credential）は guard の code のまま」と付記する（close か integrate-cite で）。
