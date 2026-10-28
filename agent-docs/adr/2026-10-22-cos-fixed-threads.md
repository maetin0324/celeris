# ADR 2026-10-22: CoS のスレッドを「統括 CoS」1 本と「案件ごとの CoS」1 本に固定する

---
tasks: [01M5JB3QMBZ6BB6J18YB6BBC0J]
---

- 日付: 2026-10-22
- 状態: 採用・実装済み
- 関連（この ADR が置き換える部分を各 ADR の冒頭に付記した）:
  [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md)（人が会話を作る D1/D2 の部分）、
  [2026-10-06-cos-chat-run-dispatch](2026-10-06-cos-chat-run-dispatch.md)（session 継続は変えない）、
  [2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（受信箱スレッド 1 本の前提）、
  [2026-10-07-cos-inbox-thread-conversation](2026-10-07-cos-inbox-thread-conversation.md)（`kind='inbox'` と origin card）、
  [2026-10-08-cos-chat-prompt-cache](2026-10-08-cos-chat-prompt-cache.md)（変えない）、
  [2026-10-15-root-done-cos-summary](2026-10-15-root-done-cos-summary.md)（D1 の root ごとの専用スレッド）、
  [2026-10-16-cos-inbox-escalated-resolve-on-source-close](2026-10-16-cos-inbox-escalated-resolve-on-source-close.md)（`cos_notification_routes` は変えない）

## 背景

人の決定（2026-10-22）: CoS について毎回新しいスレッドが作られるのは使いづらすぎる。統括・一般の CoS と案件（project）ごとの
CoS でスレッドを管理し、案件の通知はその案件の CoS に、案件に紐づかないものは一般の CoS に流す。各 CoS はセッションをなるべく維持する。

事実（base `4ccb265c`）:

- `chat_threads.kind` は `human`／`inbox`／`legacy`。`human` は人が `POST /chat/threads` で会話ごとに作り、
  root の done ごとに `root-done:<task>:<seq>` の `human` スレッドが 1 本ずつ作られていた（2026-10-15 D1）。
- 受信箱の項目（`cos_inbox_items`）は全件が 1 本の `kind='inbox'` スレッドに積まれ、triage run もそこで動く。
  案件の区別は無い。origin card は CoS operation が触れた別の `human` スレッドへ参照カードを置いていた。
- CoS の harness session は `node_sessions(kind='cos_chat', thread_id)` に DB で持ち、resume・rollover（summary＋未要約履歴で fresh）・
  resume 拒否後の fresh 再試行は `cos_chat/rollover.rs` が既に行う。session はスレッドに紐づくので、スレッドが増えると session も分かれる。

## 決定

破壊的変更の方針に従う。旧い経路（人が会話を作る・root ごとのスレッド・受信箱スレッド・origin card）は同じ変更で消す。

### D1 スレッドの種類

`chat_threads.kind` は次の 3 つだけにする（CHECK で固定。migration 0092 で表を作り直す）。

| kind | 本数 | 作る時 | 題名 |
|---|---|---|---|
| `general` | 全体で 1 本（部分 unique index） | 最初に要る時（初回の通知・人が開いた時） | 「統括 CoS」 |
| `project` | `project_id` ごとに 1 本（部分 unique index、`project_id` 必須） | その案件の初回の通知か、人が案件の CoS を開いた時 | 「CoS: <案件の題名>」 |
| `legacy` | 移行で残した旧スレッド | migration 0092 だけが作る（読むだけ） | 旧題名のまま |

スレッドを作る関数は store の `cos_thread_ensure_tx(project_id)`（無ければ作り、あれば返す）だけにする。
`project_id` が無い・案件が存在しないときは `general` を返す。

`legacy` は読むだけ: 人の `POST /chat/threads/{t}/messages` も CoS の書き込み（`chat_message_post_cos_tx`）も
409 `chat_conflict` で断る（`client_message_id` の再送は従来どおり同じ発言を返す）。web のホームは legacy を選ぶと
composer の代わりに「読むだけ。続きは統括 CoS か案件の CoS で」の注記を出す。legacy だけが保管（archived）できる。
旧 Console の `messages` 行の投影（`backfill_legacy_cos`。ADR-0054 の一度きりの取り込み）は、migration 0092 が残した
`legacy:global`／`legacy:project:<id>` があればそこへ、無ければその scope の固定 CoS スレッドへ入れる。legacy スレッドを
作る経路はコードに無い（`legacy_thread()`・`chat_legacy_default_thread`・`chat_legacy_new_conversation(_tx)` は消した）。

### D2 人の「新しい会話」

人が新しいスレッドを作る機能は**残さない**。`POST /api/v1/chat/threads` は本文 `{"project_id": <id|null>}` で
「統括または案件の CoS スレッドを開く」（冪等。初回だけ 201）に意味を変える（`title`・`client_thread_id` は未知 field として 400）。
話題が変わっても同じスレッドで続け、長さは D4 の compact に任せる。web の「新しい会話」は「案件の CoS を開く」に置き換える。

旧 Console の「新しい会話」も残さない: `POST /api/v1/console/new-conversation`（ADR-0054 D1・api.md §3.109）と
CoS operation `console.new_conversation`、web の Console の「新しい会話」ボタンと会話 id の切り替え（`conversationStore`）を消す。
Console の cache key は `['console', scope]` になり、scope の最初から読む。

### D2' Console・org 経由の人の入力

`POST /console/instruct`（scope が CoS のもの）と `POST /org/cos/messages`（互換 facade。ADR 2026-10-05 の legacy facade）は、
従来の scope ごとの legacy thread ではなく **その scope の固定 CoS スレッド**（`scope=project:<id>` → 案件 CoS、それ以外 → 統括 CoS）に
入力を積む（`task_ops::conversation::start_console_cos` → `SqliteStore::chat_console_enqueue` → `cos_thread_ensure_tx`）。
旧応答の `message_id`／`task_id`／`node_id` と `legacy_message_id` の対応（`GET /console` の重複排除、MCP `console_reply`）は変えない。
CoS はその入力を統括・案件の CoS と同じ session で扱う。MCP `console_instruct` は既に同じ規則（`thread_id` 省略時は固定スレッド、
legacy は 409）。
CoS operation の action 名 `chat.thread_create`（`POST /chat/threads`）は経路表（2026-10-09-cos-operations-all-mutations）と一致させるため名前を残し、
意味は同じく「開く」である。

### D3 振り分け

- 受信箱の項目: `CosTriageSource.task_id`（受信箱項目の task、無ければ blocking の root／task、通知なら notice の task）から
  root を辿り（`tasks.parent_id` の再帰）、root の `project_id` があればその案件の CoS スレッド、無ければ統括 CoS スレッドへ入れる。
  `cos_inbox_items.thread_id` がその行の行き先で、claim・digest・退避の hand-off 行・run の文脈（未解決項目）はすべてこの列で
  スレッドごとに分ける。triage run はスレッドごとに 1 本まで（同じ thread の run 中は待つ）。
- root 完了の要約: `root_done_summaries` の root の `project_id` で同じ規則。入力 message の `client_message_id`
  （`root-done:<task>:<seq>`）で冪等。新しいスレッドは作らない。
- 人への外部通知（Discord の escalation・fallback・root 完了）: 本文の先頭に「[統括 CoS]」または「[CoS: <案件の題名>]」を付ける
  （`chat::cos_thread_label`。escalation・fallback は item の `thread_id`、root 完了は書き込み先スレッドから引く）。
  root 完了・fallback のリンクは書き込み先スレッドを指す。
- origin card（CoS operation が触れた別スレッドへの参照カード）は消す。項目は最初から案件のスレッドに入るため。
- 統括 CoS の見渡し: 統括 CoS の run の文脈には、全スレッドの未解決項目（従来どおり件数上限あり）を渡す。案件 CoS の run には
  その案件のスレッドの未解決項目だけを渡す。web の一覧は全 CoS の未読・要対応数を出す（D5）。

### D4 セッションの維持

- 変えない部分: session は `node_sessions(kind='cos_chat', thread_id)` に DB で持ち、daemon の再起動・release の昇格をまたいで
  同じ session id で resume する（dispatcher のメモリに持たない）。`[sessions] rollover_tokens` 超過・context 枯渇は
  同じスレッドで retire → summary＋未要約履歴の fresh session にする（compact）。resume 拒否（失効）は同じ run・同じスレッドで
  fresh に 1 回だけ再試行する。
- 変わる部分: スレッドが固定されるので、案件・統括の CoS は 1 本の session を使い続ける。新しいスレッドで session を作り直す経路は無い。

### D5 既読と要対応の数

- `chat_threads.read_through_seq`（migration 0092）を足し、`POST /api/v1/chat/threads/{t}/read` `{"through_seq": n}` で上げる（下げない）。
- `ChatThread` に `unread_count`（`read_through_seq` より後の assistant・system 発言の数）と
  `attention_count`（そのスレッドの `cos_inbox_items` で `pending`／`running`／`escalated`／`fallback` の数）を足す。
- web のホームは統括 CoS を先頭に、各案件 CoS を一覧し、選ぶと切り替わる。数を badge で出す。案件の画面に「この案件の CoS」への導線を置く。

### D6 既存データの移行（migration 0092）

- `inbox` → `general`（同じ id。題名を「統括 CoS」に。messages・runs・session・items はそのまま）。`inbox` が無ければ空の `general` を作る。
- `human` と旧 `legacy` → `legacy`（id・messages・runs・session・status はそのまま。読むだけ）。
  root 完了の旧スレッド（`root_done_summaries.thread_id`）も `legacy` になり、行の参照はそのまま残る。
- `pending`／`running` の受信箱項目は `general` に付いたまま（再振り分けしない。旧項目の task は分からないものがある）。
- 失うデータは無い。FTS の trigger は作り直す。`read_through_seq` は移行時点の `next_seq - 1` から始める（移行前の発言を未読にしない）。

### D7 置き換えるもの（消すもの）

`ChatThreadKind::{Human, Inbox}`、`ChatCreateThreadRequest`（→ `ChatOpenThreadRequest`）、`SqliteStore::chat_thread_create`（→ `cos_thread_open`）、
受信箱スレッドの探索（dispatcher の `inbox_thread()`・`CosTriageState.inbox_thread`・`inbox_run_live`）、origin card、
root 完了ごとのスレッド作成、`cos_triage_runs_without_digest` の inbox 固定、`instructed_by` の「受信箱スレッドだけ」の制約
（→ `legacy` 以外の CoS スレッド）。web の新規会話作成と「受信箱」スレッドの特別扱い。

D1／D2／D2' で消したもの: `POST /api/v1/console/new-conversation`（route・`new_conversation` handler・`new_conversation_project`）、
CoS operation `console.new_conversation`（surface registry と ADR 2026-10-09 の表から削除）、store の `legacy_thread()`・
`chat_legacy_default_thread`・`chat_legacy_new_conversation(_tx)`・`feed_cursor` の `cos_chat_legacy_default:*` 鍵、
`start_legacy_cos`／`chat_legacy_enqueue`（→ `start_console_cos`／`chat_console_enqueue`、固定スレッドへ）、
web の `startNewConversation`・`useNewConversation`・`conversationStore`・Console の「新しい会話」ボタン、
`web/e2e/parity/console.spec.ts` の new-conversation parity と fake daemon の応答。

### D8 本番への移行

config の変更は無い。migration 0092 は release の昇格で自動適用される。

## 付記（2026-10-22、review の差し戻しへの対応）

初回 review の指摘: `legacy_thread()` が `kind='legacy'` を作り続け（`POST /console/new-conversation`・`console.new_conversation` は
rotate=true で毎回新しいスレッド）、`POST /org/cos/messages`・`POST /console/instruct` は scope ごとの legacy thread に入り CoS が別 session で
動いていた。legacy は「読むだけ」と書きながら `message_post_conn` は断らず、web の composer も有効だった。対応は D1（legacy の作成経路なし・
読むだけの強制）・D2（Console の「新しい会話」廃止）・D2'（Console・org の入力を固定スレッドへ）・D7 のとおり。試験:
task-api `console_instruct.rs`（Console・org の入力が統括／案件の CoS スレッドに入りスレッド数が増えない）、`chat_api.rs`
`chat_api_legacy_thread_is_read_only`、task-core `chat_store_legacy_thread_is_read_only`・
`chat_console_projection_lands_on_fixed_cos_threads_without_legacy_threads`・
`chat_console_projection_reuses_legacy_threads_kept_by_migration_0092`、web `conversation-input.test.tsx`。

## 付記（2026-10-28、[2026-10-28-single-human-window-general-cos](2026-10-28-single-human-window-general-cos.md)）

人が話す窓口は統括 CoS の 1 本にする。D1（固定スレッド）と D4（session はスレッドごと）は変えない。
案件スレッドの item が人へ回されると、統括スレッドに札（source triple で冪等、元の件の state から解消表示）が出る。
D5 の「web のホームは統括 CoS を先頭に各案件 CoS を一覧し、選ぶと切り替わる」は置き換えた: ホームは統括 CoS を開き、
案件スレッドは札の参照と案件の画面から `/?thread=<id>` で開く読む用の導線だけにする。
