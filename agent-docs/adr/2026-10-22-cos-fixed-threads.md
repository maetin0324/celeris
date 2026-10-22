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
| `legacy` | 移行で残した旧スレッド | 作らない（読むだけ） | 旧題名のまま |

スレッドを作る関数は store の `cos_thread_ensure_tx(project_id)`（無ければ作り、あれば返す）だけにする。
`project_id` が無い・案件が存在しないときは `general` を返す。

### D2 人の「新しい会話」

人が新しいスレッドを作る機能は**残さない**。`POST /api/v1/chat/threads` は本文 `{"project_id": <id|null>}` で
「統括または案件の CoS スレッドを開く」（冪等。初回だけ 201）に意味を変える（`title`・`client_thread_id` は未知 field として 400）。
話題が変わっても同じスレッドで続け、長さは D4 の compact に任せる。web の「新しい会話」は「案件の CoS を開く」に置き換える。
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

### D8 本番への移行

config の変更は無い。migration 0092 は release の昇格で自動適用される。
