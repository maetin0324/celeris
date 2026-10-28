# ADR 2026-10-28: 人が話す窓口を統括 CoS の 1 本にし、案件 CoS の escalate を統括へ札で集め、統括から案件へ取り次ぐ

---
tasks: [01M60ABQN55C65R4W13ZS0347F]
---

- 日付: 2026-10-28
- 状態: 採用・実装済み
- 関連（各 ADR に付記した）:
  [2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)（固定スレッドと session 分離は変えない。D5 の web の導線を置き換える）、
  [2026-10-27-relayed-instructions](2026-10-27-relayed-instructions.md)（取り次ぎの主体に統括 CoS を足す）、
  [2026-10-16-cos-inbox-escalated-resolve-on-source-close](2026-10-16-cos-inbox-escalated-resolve-on-source-close.md)（札の解消はこの規則に従う）、
  [2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)（CoS operation の表に 1 行足す）、
  [2026-10-27-project-brief](2026-10-27-project-brief.md)（統括 CoS が読む案件の状態の要約 C1）

## 背景

利用者（2026-10-28）: 「案件ごとにこちらで首振りをするのも大変なので、スレッドが一個のほうが楽」。

10-21 以降の 1 週間（本番 DB の読み取り）の発言数は、自己改善 867（人 122）、統括 529（人 1）、moshi 129（人 5）、
BenchFS 92（人 1）。大半は受信箱の項目と一次対応の system 行である。全案件を 1 本のスレッドと 1 本の session に
まとめると、次の問題が出る。

- `[sessions] rollover_tokens`（400k）の rollover が頻発し、要約で案件の文脈が落ちる。
- 案件の指示が混ざる。
- run が 1 本に直列になり、他の案件の返事が遅れる。
- turn ごとの入力が増える。

そのため session とスレッドは統合しない。人が話す窓口だけを統括 CoS の 1 本にする。

## 決定

### D1 窓口は統括 CoS、文脈は案件ごと

- 2026-10-22-cos-fixed-threads の D1（統括 1 本＋案件ごと 1 本の固定スレッド）と D4（session はスレッドごと）は変えない。
  案件の文脈（session、履歴、受信箱の一次対応）は案件スレッドに残る。
- 人は統括スレッドだけで話す前提にする。案件スレッドは読むためのもので、札（D2）の参照と案件の画面から開く。
  人が案件スレッドに書くことは禁止しない（composer は残す）が、web の主導線にはしない（D5）。
- 統括 CoS は案件の状態を案件の状態の要約（C1、`project_briefs`。prompt の案件一覧に載る）で読み、詳しく要る案件だけ
  その案件スレッドの履歴を読む。全案件の履歴を毎回読まない。

### D2 人へ回す件は統括スレッドに札として出す

- 案件スレッド（`kind='project'`）の `cos_inbox_items` が人へ回されたとき、同じ transaction で統括スレッドに札を 1 件書く。
  人へ回す経路は escalation・fallback の outbox を作る 2 か所（`cos_triage_outbox_claim`：CoS の escalate と
  CoS 不在の fallback、`cos_triage_direct_escalate`：credential_wait などの直送）で、どちらも札を書く。
  question・decision・approval・plan gate などの種類は区別しない（`source_kind` を札に載せる）。
  統括スレッドの item（案件の無い件）は既に統括スレッドにあるので札を書かない。
- 札は統括スレッドの `role=assistant`・`completed` の発言で、run の入力にならない（CoS run を起こさない）。
  `client_message_id` は `cos-escalation-card:<source_kind>:<source_key>:<source_revision>`（source triple）で、
  同じ件の 2 回目は何も書かない（部分 unique index `idx_chat_messages_client` で冪等）。
- 札の本文は短く、案件名・task・何を決めるか（outbox の `summary`）・選択肢（推奨）・元のスレッドの名を書く。
  metadata の `escalation` に `item_id`・`source_kind`・`source_key`・`source_thread_id`・`project_id`・`task_id` を持ち、
  API の `ChatMessage.escalation`（`ChatEscalationRef`）として出す。`cards` には件の種類のカード（decision → decision、
  question → question、approval／authorization → approval、phase_gate → plan_gate、他は notice）を 1 枚載せ、
  id は受信箱の項目 id（`source_key`）、href は outbox の `web_path`。
- 札の状態は保存しない。読むたびに `cos_inbox_items.state` から決める（`escalation.state` に元の state、
  `escalation.open` は `pending`／`running`／`escalated`／`fallback` のとき true。カードの `state` は open なら元の state、
  閉じたら `resolved`）。元の件の解消は 2026-10-16 の規則（`resolve_stale`：現在の未解決 snapshot に無い件を resolved）の
  とおりで、札の側に別の解消経路を作らない。events は書かない（chat の message であり、chat_events に出るだけ）。
- 案件スレッドへの記録（item、digest、一次対応の system 行）は変えない。

### D3 統括から案件への取り次ぎ（`chat.relay_forward`）

人が統括スレッドで答えた発言を、統括 CoS が案件スレッドへ取り次ぐ CoS operation を足す。

- route: `POST /api/v1/chat/threads/{t}/forwards`（`{t}` は取り次ぎ先の案件スレッド）。CoS operation
  （`/cos/operations` の `request`）としてだけ受け付ける。直接の呼び出し（人・operator・MCP の token）は
  403 `chat_forward_cos_only`。人は自分の発言を `…/messages` に、operator は `…/relays` に書く。
- body: `{client_message_id, message_id, task_ids, expires_at}`。`message_id` は統括スレッドの発言で、次をすべて満たす。
  - 呼び出した run が統括スレッドの run である（案件スレッドの CoS は取り次がない）。
  - `role=user`、取り消されていない、`author` が `human:<name>`（認証された人の発言）、それ自体が取り次ぎ（`relay`）でない。
- 本文は受け取らない。取り次ぎの発言の本文は `message_id` の原文をそのまま写し、`said_at` はその発言の `created_at`。
  これで統括 CoS は自分の判断を人の発言として取り次げない（D3 の主要な制約）。
- 範囲: `task_ids` は 1〜20 件で必須、すべて取り次ぎ先の案件の task。`expires_at` は必須で、今より後。
  案件全体・無期限の取り次ぎは統括 CoS からはできない（必要なら人が案件スレッドに書くか operator が `…/relays` を使う）。
- 記録: 取り次ぎ先の案件スレッドに、2026-10-27-relayed-instructions D1 と同じ `role=user`・queued の発言を作る。
  `author` と `relay.relayed_by` は `cos:<統括スレッド id>`、`relay.source` に元の `{thread_id, message_id}`。
  `client_message_id` は body の値で、同じ値・同じ内容の再送は同じ発言を返す。
  発言は queue に入り、案件 CoS の run の入力になる。
- 監査: 取り次ぎの operation は統括スレッドの `cos_operations` に `chat.relay_forward` として残る（actor cos、
  target `chat_thread` = 取り次ぎ先）。案件 CoS が取り次ぎを `instructed_by` に引いた operation の payload は
  `decided_by = {kind: human, id: <元の author の name>, relayed_by: {kind: cos_general, id: <統括スレッド id>}}`、
  `executed_by = {kind: cos_project, id: <案件スレッド id>}` になり、判断者・取り次ぎ・実行者が分かれる。
- 案件 CoS は取り次ぎを根拠に既存の operation（`inbox.answer`、retry など）を実行する。範囲外の task・期限切れの取り次ぎは
  2026-10-27 D3 のとおり 422 `cos_instruction_invalid`（rejected で監査に残る）。

### D4 二重回答を防ぐ排他

- 統括 CoS は案件の件に自分で回答 operation を実行しない。`instructed_by` を付けた operation で、呼び出しが統括スレッドの
  run で対象（task、または受信箱の件の task）が案件に属するとき、422 `cos_instruction_invalid`
  （「統括 CoS は案件の件を取り次ぐ。`POST /chat/threads/{t}/forwards` を使う」）。案件の件に答えるのは案件 CoS だけになる。
- 人が札のボタンで直接答える場合は人の回答（`POST /inbox/items/{id}/answer`、actor は人）で、CoS は関わらない。
  その後に取り次ぎが届いても、元の件は閉じているので案件 CoS の回答は領域 API が 404／409 で断る（領域の既存の排他）。
- 同じ発言を同じ案件へ 2 回取り次がないよう、統括 CoS は `client_message_id` を `forward:<message_id>:<案件スレッド id>` に
  する（skill に書く）。同じ値は同じ発言を返す。

### D5 web

- ホーム（`/`）は統括 CoS を開く。スレッドを選ぶ一覧（統括＋全案件の並び）と、選んだスレッドへの切り替えはホームから消す。
- 案件スレッドは `/?thread=<id>` で開ける（札の「元のスレッド」と案件の画面の「この案件の CoS」から）。案件スレッドを
  開いているときは「統括 CoS に戻る」を出す。
- 札は統括スレッドの発言として既存のカード描画で出し、解消したら閉じた状態で出す。「元のスレッドを開く」のリンクを付ける。
- 2026-10-22-cos-fixed-threads D5 の「ホームは統括 CoS を先頭に各案件 CoS を一覧し、選ぶと切り替わる」はこの D5 で置き換える
  （同じ変更で消す）。

### D6 統括 CoS の prompt・skill

- `cos-inbox-triage` skill（policy_version 5）に、統括 CoS の札の答え方と取り次ぎ方を書く: 人が統括で札に答えたら
  `chat.relay_forward` で案件スレッドへ取り次ぎ、自分では回答 operation を実行しない。範囲は札の task、期限は既定 24 時間。
- `cos-operator` skill の operation 表に `chat.relay_forward` を足す。
- 統括 CoS の固定 Core（バイト上限あり）は変えない。

### D7 本番への移行

config の変更は無い。DB の migration も無い（札は既存の `chat_messages`、取り次ぎは既存の metadata）。
既存の escalated item には札を後から作らない（新しく人へ回された件から出る）。

## 検証

- task-core: 案件スレッドの item を escalate すると統括スレッドに札が 1 件だけ出て、再 claim・直送の 2 回目で増えない。
  統括スレッドの item には札が出ない。元の件が resolved になると札は `open=false`・カードは `resolved` で読める。
- task-api 通し試験（LLM なし、run credential で CoS の run を模す）: 案件 CoS が escalate → 統括に札 → 人が統括で答える →
  統括 CoS が `chat.relay_forward` → 案件スレッドに取り次ぎの発言 → 案件 CoS が `instructed_by` 付きで回答 →
  reconcile で元の件が resolved → 札が解消表示。範囲外の task・期限切れの取り次ぎ、統括 CoS の直接回答、
  人の token の forwards、統括以外の run の forwards、人以外の発言の forwards は拒否。
- web: ホームが統括 CoS を開き、スレッドの一覧が無いこと、`?thread=` で案件スレッドを開けること、札の描画。
