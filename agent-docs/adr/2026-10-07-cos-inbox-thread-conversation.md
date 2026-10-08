# ADR 2026-10-07: 受信箱スレッドを「受信箱の件を CoS と話す場所」にする

---
tasks: [01M4CAKADGDTZA7QKDJ9GX35MW]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D3/D5/D6、[2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（store 契約）、[2026-10-06-cos-inbox-triage-fallback](2026-10-06-cos-inbox-triage-fallback.md)（D6 退避）、[2026-10-07-cos-live-fixes](2026-10-07-cos-live-fixes.md)

## 1. 文脈

人の指摘（2026-10-07）: CoS チャットで会話を分けられるのはよいが、受信箱に来た内容をどこで聞き、どこで答えればよいか分からない。
人の決定: 「受信箱」スレッド（`chat_threads.kind='inbox'`）を受信箱の件について話す**唯一の場所**にする。件ごとの会話は作らない。

現状: triage（`crates/task-core/src/chat/triage.rs`、`crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`）は件が来るたびに受信箱スレッドへ
system 行「CoS 受信箱の一次対応: item_ids=[…]」を積んで CoS run を起こす。run は completed になるが、triage run には出力 message が無く、
assistant の発言が 1 件も残らない。判断（何の件か・answer/escalate/observe・理由）は `cos_inbox_items.reason` にしかない。D6 の退避行も
「item_id=… は人へ委ね済み」だけで、何の件かが分からない。人がこのスレッドに書き込んだときの扱いも決まっていない。

## 2. 決定

### D1. CoS の判断は dispatcher が決定的に assistant 発言へ整形する（CoS に書かせない）

- triage run（入力が system message の run）が終端になるたび、dispatcher の tick が **1 run につき 1 件**の assistant 発言を受信箱スレッドに残す。
  冪等 key は `chat_messages.client_message_id = cos-triage-digest:<run_id>`（system の keyed message と同じ仕組み。role だけ assistant）。
  `run_id` も付けるので、どの run の判断かが残る。
- 材料は SQLite だけ（LLM 無し。ADR cos-chat-home D3「dispatcher は推論しない」と同じ境界）: `cos_inbox_items` の `state`・`reason`・`operation_id`・
  **新列 `summary`**（件の題名。投入時に `CosTriageSource.summary` を保存する。migration 0059）、escalate の outbox 本文
  （`cos_notification_routes` → `notifications.body` の JSON: `summary`・`options`・`recommended`・`recommendation_reason`・`web_path`）。
- 件ごとに書く: 「何の件か」（種類の日本語名 + 題名）、「判断」（代わりに答えた / 人に回した / 見ただけ / 未処理 / 解決済み / 中断）、
  「理由」（`reason` の要点。無ければ「理由の記録なし」）、人に回した件は「人が決めること」（packet の summary）と「選択肢」（label の列、推奨に印）。
- カード: 人に回した件は答えられるカード（`kind` は source_kind から、`id` = source_key = 受信箱の項目 id、`state=escalated`、`href` = packet の
  `web_path`、`actor=cos`、`reason` = 推奨とその理由）。web は既存の chat card（ADR cos-chat-home D5）で `escalated` を人待ちとして数え、
  受信箱 API の選択肢で答える。答えた件は受信箱 API が 404 を返すので閉じた表示になる。answer / observe の件は閉じた状態のカード
  （`state=answered` / `observed`）で残す。
- CoS 自身の最終文は使わない。triage run には出力 message が無く（`text_rejected`）、1 件ずつの判断は `reason` に書かせる設計（skill §3）で
  十分だから。CoS が reason を書かなかった場合も「理由の記録なし」として件は必ず残る（受け入れ条件 0 の「決定的に補う」）。
- 終端が completed なのに `running` のまま残った件は「未処理（人へ直接通知する）」と書き、処理は D6 の退避に任せる。
- 回収: 終端済みで digest の無い triage run は、run の終了を観測した tick と起動時の reconcile で拾う（上限 20 run/tick）。
  対象は件を 1 つ以上持つ run だけなので、導入前の古い run にも 1 度だけ digest が付く（以後は増えない）。

### D2. 退避（fallback）の行は題名と「人が決めること」を書く

- handoff message の本文を「CoS 不在のため直接通知: 『<題名>』（<種類>）は人へ委ねた。決めること: <要点>。選択肢: a / b。回答は <web_path>」にする。
  カードの title は題名、`href` は `web_path`、`state=pending`（人待ち。元の待ちが開いている限りカードから答えられる）、`reason` は不在理由。
- item id・revision は本文に残さない（カード id が source_key なので追える）。

### D3. 人は受信箱スレッドに書き込める。CoS には未解決の件を文脈として渡す

- 投稿は既存の `POST /chat/threads/{t}/messages` のまま（受信箱スレッドだけ禁じていた箇所は無い）。人の発言は通常の queue claim で run になる。
- 受信箱スレッドの run（triage・人の発言とも）の `CosChatContext` に `inbox_items: Vec<CosChatInboxItem>` を足す
  （serde default。他スレッドでは空）。中身は未解決の件: `escalated`（人待ち）、`fallback`（人へ委ね済み）、`pending`/`running`（CoS 未処理）を新しい順に
  最大 50 件。各件に `item_id`・`source_kind`・`source_key`・`source_revision`・`state`・`summary`・`reason`・`created_at`・`decision`
  （packet の `summary`・`options`・`recommended`・`web_path`）・`answer_path`（人の回答と同じ `POST /api/v1/inbox/items/{source_key}/answer`）。
  worker の prompt に「## 受信箱の未解決の件」節として出す。直近のやり取りは既存の `unsummarized` 履歴で渡る。
- 人の自由文（「(b) にして、ただし host 1 台だけで」）への応答: CoS は該当の件を文脈から選び、`/cos/operations` で
  `POST /api/v1/inbox/items/{id}/answer`（**新規 ALLOWED、action `inbox.answer`**。`delegated_request` で元の領域 API へ委ね、
  同じ検証を通す）を呼ぶ。本文に **`instructed_by`**（人の発言の message id）を付ける。API は **呼び出し元の run（bearer の
  `run_id`）から** 次を確かめる（1 つでも違えば 422 `cos_instruction_invalid`、operation は `rejected` として記録）:
  (1) そのスレッドの message であること、(2) `role=user` であること、(3) スレッドが `kind=inbox` であること、
  (4) その message が **この run の入力（`chat_runs.input_message_id`）** であること。
  これで、一次対応の run（入力は system 行）、スレッドの過去の無関係な発言（「こんにちは」）、他のスレッドの発言、通常の会話スレッドの
  発言では human_required を外せない。LLM の申告ではなく、run と人の発言の結び付き（人が書いた → その発言で run が起きた）を API が確かめる。
  終端した run の bearer は credential の検査で先に拒まれる。
  通れば operation の payload に `instructed_by: {message_id, seq}` を、監査の reason に
  「人の指示（seq N）: …」の接頭辞を記録する。actor は wire 上 `cos` のまま（worker が人を名乗る経路は作らない。ADR D3）が、
  指示の出所は payload と reason で追える。`instructed_by` 付きの operation では **明示 human_required の拒否を外す**
  （決めたのは人で、CoS は伝えただけ。人の発言 1 つにつき run は 1 つで、その run だけが外せる）。`instructed_by` の無い operation は従来どおり。
  ADR cos-chat-home の「人が設定した human_required は skill で解除できない」は保たれる: 解除の根拠は人の発言そのものであり、
  CoS が選べるのは「その発言をどの件への回答と読むか」だけ（誤読は領域 API の検証と監査で追える）。
- どの件か曖昧なら CoS は operation を出さず、返事で聞き返す（skill §9）。

### D4. 並び順と取りこぼし

- 同じ tick では `claim_queued`（人の発言）が `triage_launch` より先に走る。人の発言と件の到着が重なれば人の発言の run が先。
  active run が 1 つの制約は両 claim がそれぞれ確かめる（queue claim は `active_run_id`、triage claim は `chat_runs` の running/stopping）。
- triage run の実行中に人が書けば message は queue に積まれ、run の終端後の tick で先に run になる。件は `running` のまま待ち、
  その後の tick で `dirty` により claim される。
- 人が「割り込んで送信」で triage run を止めた（stopped / interrupted）場合、D1 の digest 時にその run の `running` の件を `pending` に戻す
  （`cos_triage_requeue_run`）。退避の期限は `created_at` 起点なので無限には回らない。completed の取り残しは従来どおり退避へ。

### D5. web

- 人待ちのカードを持たない system 行（triage の入力行・知らせ・閉じた退避の行）を折りたたみ（`<details>`。1 行目だけ見せる）で出す。
  人待ちのカード（`pending` / `escalated` の decision/question/approval/plan_gate）を持つ発言と、人・CoS の発言は通常の表示。
  判定は message の role と cards だけで決まるので、thread の種類に依らず同じ（system 行が人待ちを持つのは受信箱スレッドだけ）。
- composer は受信箱スレッドでも使える。placeholder を「受信箱の件について CoS に聞く・指示する」にする。

## 3. 帰結

- dispatcher・store に LLM は入らない（整形は文字列の組み立て）。
- `cos_inbox_items.summary` が増える（migration 0059、既存行は空文字。空なら digest は `source_kind:source_key` で代用する）。
- `CosChatContext` と `OperationBody` に任意欄が増える（schema 再生成）。

## 4. 試験

- store: `cos_triage_report_*`（run の件の報告と packet の取り出し）、`cos_triage_open_items_*`、`chat_assistant_message_add_once_*`。
- dispatcher: `cos_chat_triage_digest_*`（終端後に assistant 発言が残る・reason 無しでも残る・escalate のカードが `escalated`）、
  `cos_chat_triage_fallback_*`（handoff 文に題名と選択肢）、`cos_chat_triage_requeue_on_interrupt`、
  `cos_chat_inbox_human_message_*`（人の発言で run が起き、`inbox_items` が文脈に入る。triage と重なっても両方処理）。
- task-worker: `cos_chat_prompt_inbox_items_section`。
- task-api: `cos_chat_inbox_thread_instructed_by_*`（検証・記録・human_required の解除。`refuses_messages_the_run_does_not_answer` が一次対応 run・過去の発言・他スレッド・会話スレッドの 422 を固定）、`cos_operation_inbox_answer_*`。
- web: vitest `chat_inbox_*`（折りたたみ・人待ちは通常表示・placeholder）。

## 付記: 実装との突き合わせ（2026-10-07）

- D1: `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`（`digest_message` は純粋。`triage_digest` は tick の `triage_ingest` で
  `digest_due` のときだけ走る。`digest_due` は reconcile と、受信箱スレッドの run が `running` から消えたとき（`inbox_run_live`）に立つ）。
  store は `cos_triage_run_report` / `cos_triage_runs_without_digest` / `chat_assistant_message_add_once`（`crates/task-core/src/chat/{triage,store}.rs`）。
  migration `0059_cos_inbox_item_summary.sql`（`summary` 列と `run_id` の index）。digest の冪等 key は `cos-triage-digest:<run_id>`。
  run の終端と次の tick の間に process が落ちると digest は再起動時の reconcile で書かれる（件を持つ run だけ）。
- D2: `fallback.rs` の `handoff_text`（純粋）。カードは `state=pending`・`href` は packet の `web_path`。
- D3: `CosChatContext.inbox_items`（`crates/task-worker/src/protocol/cos_chat.rs`）を `launch.rs` が受信箱スレッドの run に入れる（上限 50）。
  prompt は `crates/task-worker/src/cos_chat.rs` の `inbox_section`。API は `OperationBody.instructed_by`・`ALLOWED` の `inbox.answer`
  （`crates/task-api/src/cos/operations.rs`。`delegated_request` で領域 API へ）。skill は `config/skills/cos-inbox-triage/SKILL.md` §9。
  `instructed_by` の検査は `instructed_message`（同 file）: message の存在と `role=user` → thread の `kind=inbox` → `chat_run_get(thread, CosCaller.run_id)` の
  `input_message_id` と一致、の順。却下は `audit.reject` で `cos_operations` に `rejected` として残る（2026-10-08 の差し戻しで追加。
  それまでは「同じスレッドの role=user」だけを見ていて、一次対応 run や過去の無関係な発言の id で human_required を外せた）。
  決定との差分: 「どの件か曖昧なら聞き返す」は skill と prompt の指示であり、API は強制しない（判断は CoS、検証は API という D3 の境界のまま）。
- D4: 中断された run の件の戻しは digest の中（`cos_triage_requeue_run`）。completed の取り残しは従来どおり退避。
- D5: `web/features/chat/messages/message-item.tsx`（`foldedSummary`・`data-slot="chat-system-folded"`）、
  `web/features/chat/composer/chat-composer.tsx`（`placeholder`・`INBOX_COMPOSER_PLACEHOLDER`）、`chat-home.tsx` が受信箱 thread で渡す。
- 試験: 本文 §4 のとおり（名前は `cos_chat_triage_digest_*`・`cos_chat_triage_human_interrupt_*`・`cos_chat_inbox_*`・
  `cos_chat_prompt_inbox_items_*`・`cos_chat_triage_store_report_*`・`cos_chat_inbox_thread_instructed_by_*`・web の `chat_inbox_*`）。
