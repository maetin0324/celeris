# ADR 2026-10-28: 受信箱の件をチャットに出すとき、切れた本文のままにせず CoS が要約を補う

---
tasks: [01M61HWT7Q8CPF0WTYX5R7NM71]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-07-cos-inbox-thread-conversation](2026-10-07-cos-inbox-thread-conversation.md)（D1 digest・D2 退避の行・D3 文脈）、
  [2026-10-06-cos-inbox-triage-fallback](2026-10-06-cos-inbox-triage-fallback.md)、
  [2026-10-28-single-human-window-general-cos](2026-10-28-single-human-window-general-cos.md)（統括スレッドの札）

## 1. 文脈

人の指摘（2026-10-28、統括 CoS チャット seq 3936）: 受信箱の件がチャットに出るが、途中で切れていて読めない。

調べた結果、チャットに件が出る経路は 4 つある。

| 経路 | 書く主体 | 本文の出所 | 切れていた所 |
| --- | --- | --- | --- |
| 退避の行（CoS 不在で直接通知） | daemon（`fallback.rs` `handoff_text`） | `InboxItem.detail` | `human_inbox::clip` が 600 字で切る。認可の選択肢 A〜C は question の後ろにあり落ちる |
| 一次対応の digest（CoS が人に回した件） | daemon（`digest.rs`） | CoS の escalation packet | packet の選択肢は label だけ（「今回だけ認める」）で中身が無い |
| 統括スレッドの札 | daemon（`window.rs`） | outbox packet | 同上。退避の札は題名に本文全体が入る |
| CoS の返事 | CoS | prompt の「受信箱の未解決の件」 | 題名と packet しか無く、本文は API を読みに行くしかない |

知らせ（notice）の件では題と本文が別の通知のものになる例もあった。原因: `cos_inbox_items.summary` には取り込み時の題名
（revision = 束の件数）を保存するが、退避は `notice_get` で **今の** 束を読む。束は同じ group の新しい出来事で題名・要約が
上書きされるので、「日次整理が完了」の題に別 task の完了の本文が付いた。

## 2. 決定

### D1. 機械的に切る所は全文を残す

- `InboxItem.detail` は従来どおり 600 字（`DETAIL_CLIP_CHARS`）で切る（受信箱の一覧のため）。切ったときは `links` に
  `{"label":"全文","href":"/api/v1/tasks/<id>"}` を足す。切る前の本文は `InboxItem.detail_full`（API には出さない、process 内だけ）に残す。
  統合の依頼・未取り込みの本文は従来どおり切らない。
- 退避（fallback）と直送の outbox、退避の行は `detail_full` を使う。チャットの行は 4,000 字（`CHAT_BODY_MAX_CHARS`）を超えたら
  「…（続きは /tasks/<id>）」で切る。Discord の 1,900 字の上限は従来どおりで、本文の後ろに web へのリンクが付く。

### D2. 件の題名と本文を同じ revision で保存する（notice の食い違いの修正）

- migration 0101 で `cos_inbox_items.detail` を足す。取り込み時に、受信箱の件は `detail_full`、知らせは同じ束の状態の `summary` を保存する。
- 退避は知らせの本文を保存した `detail` から作る。0101 より前の行（`detail` が空）は、今の束の件数が revision と同じときだけ今の束を読み、
  違えば題名だけを出す。束が育ったら新しい revision の行ができ、その行が新しい題名と本文を持つ。
- CoS の prompt の「受信箱の未解決の件」に、件ごとの本文（1,500 字まで）を出す。

### D3. CoS が人に回すときは選択肢ごとの中身を書く

- escalation packet の `options[]` に任意の `detail`（1〜600 字）を足す。label は短い句、`detail` は選んだら何が起きるか・何を認めるかの文。
- digest・札・prompt は `detail` があれば選択肢を 1 行ずつ「label: detail」で出す。
- skill `cos-inbox-triage` §6 に、summary の書き方（何が起きたか → 人が決めること）と `detail` の書き方を足す。

### D4. CoS 不在で人へ回した件にも、後から CoS の要約を出す

- 退避の 1 回の処理で人へ回した件（最大 20 件ずつ）について、daemon は統括スレッドに **queued の system message**
  「CoS 不在で人へ直接回した件の要約: items=[…]」を 1 件積む（冪等 key `cos-fallback-summary:<最初の item id>`）。
  本文は件ごとの題名・種類・元のスレッドと、読むべき API（`GET /api/v1/tasks/<id>` 等）。dispatcher は推論しない。
- chat の queue（`queue_order`・`queued_count`）は `role IN ('user','system')` の queued を扱う。通常の queue claim が CoS run を起こし、
  run の最終文が assistant の返事としてスレッドに残る。`instructed_by` は `role=user` を要するので、この依頼を根拠に人の判断を代えることはできない。
- 新しい一次対応の件があるスレッドでは、system の依頼より一次対応を先に走らせる（人の発言は従来どおり先）。
- CoS が無効（`cos.enabled=false`）か起動が失敗すると分かっている（`unavailable_reason`）ときは積まない。積んだ依頼の run が失敗したら
  その依頼は失敗のまま残る（再試行は人が web の再送で行う）。
- 退避の行は「CoS が戻り次第、統括 CoS のスレッドにこの件の要約を書く」と書く。

## 3. 帰結

- dispatcher・store に LLM は入らない（D4 の依頼は文字列の組み立て）。
- schema: `cos_inbox_items.detail`（migration 0101）、API `EscalationOption.detail`、worker protocol `CosChatInboxItem.detail`・`CosChatInboxOption.detail`。
- 旧い仕組みの残り: 退避が今の束を読む経路は 0101 前の行だけの読み替えに縮めた。

## 4. 試験

- task-ops: `human_inbox_long_authorization_keeps_full_text_and_link`（600 字超の認可で detail は切れ、`detail_full` は全文、`全文` link がある）。
- task-dispatch: `cos_chat_triage_fallback_hands_over_the_whole_body_and_queues_a_summary`（選択肢 C まで退避の行に出る・
  要約の依頼が queued system で 1 件・CoS が戻ると run になる・一次対応は再び走らない）、
  `cos_chat_triage_fallback_disabled_queues_no_summary`、`cos_chat_triage_fallback_notice_title_and_body_stay_together`
  （束が育っても revision 1 の退避は「日次整理が完了 — KB の受信箱を 3 件整理した」で、別 task の本文が入らない）。
- task-worker: `cos_chat_prompt_inbox_items_section`（本文と選択肢の detail）。
