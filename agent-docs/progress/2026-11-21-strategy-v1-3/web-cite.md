---
title: 戦略層 v1 子 3 web-cite — 案件の CoS chat の自分の発言に編集・撤回、要約・案の添付に引用の状態と失効の警告
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
---
# web-cite（WorkUnit。子 3 の段 web）

設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D9 のうち編集・撤回と出典の部分（D5・D6 の画面）。
基準は `$CELERIS_WU_BASE`。既存の `web/features/chat` の data・messages・home を延長した。独立した chat の実装は作っていない。
`crates/` と `web/api/generated` は変えていない（cite 段で再生成した型 `ChatMessageEditRequest`・`ChatMessageRetractRequest`・
`ChatMessageRevisedData`・`ChatCitationView`・`ChatMessage.revision/edited/retracted/citations` をそのまま使う）。

## 実装前 brief

1. 根本原因: daemon は人の発言の編集（PATCH …/messages/{m}）・撤回（POST …/retract）・前の版（GET …/revisions）と、返事の `citations`
   （引用の状態 current・edited・retracted・missing）を返す（edit-retract・summary-cite・cite-attach）が、web にはそれを呼ぶ口も表示も無い。
   SSE の `message_revised` も reducer が捨てていた。
2. 責務 owner: API の呼び出しは `data/client.ts`、expected_revision と 409/403 の解釈は `data/session.ts` の `revise`、
   表示の状態（引用の読み直し・古い版の破棄・表示している人）は `data/reducer.ts`、操作を出す発言の判定と状態の導出は `data/revision.ts`、
   表示は `messages/`（吹き出しは `message-item.tsx`）。
3. 既存の再利用先: `apiMutate`・`apiGet`・`isApiError`、reducer の `putMessage`・`pending_ack`・snapshot、`session.refresh`（再読込）、
   `components/ui` の Button・Input・Notice・Badge、`workspace-links.ts`（添付の path と attachment の対応）、生成型、既存の試験の fixture。
4. 選択肢: 引用の状態を (a) 画面で本文の sha256 から導き直す — 不採用（状態の正本が 2 つになる）。
   (b) daemon が導いた `state` を表示し、画面に出した後の編集・撤回（SSE）だけ daemon と同じ順（retracted → edited）で読み直す — 採用。
   操作を出す判定を (i) 画面で常に出して daemon の 403 に任せる — 不採用（CoS・取り次ぎの発言に操作が見える）。
   (ii) role・author・relay・state から出す発言を絞り、最終の権限は daemon の 403 — 採用。
5. 新しい状態・分岐・fallback・port: reducer の `viewer`（表示している人の主体。この画面の送信の確定で daemon の記録から知る。知るまでは null で、
   人の発言に操作を出し、他人の発言は daemon が 403 にする）と action `message_put`（編集・撤回の応答を SSE を待たずに表示に入れる）。
   `ChatSessionApi` に `editMessage`・`retractMessage`、`createChatSession` の `api` を Partial（試験で差し替える口だけ渡す）。fallback・切替 flag は無い。
6. 撤去対象: 無し（置き換える旧い仕組みが無い。`putMessage` は古い revision の message を捨てるようになったが、旧い経路は残していない）。
7. 不変条件と試験: 下の「試験」。

## したこと

- `data/client.ts`: `editMessage`（PATCH）・`retractMessage`（POST …/retract）・`listMessageRevisions`（GET …/revisions）と path。
- `data/session.ts`: `revise(message, change)`。表示している revision を `expected_revision` にして送り、成功した message を `message_put` で表示に入れる。
  409 → conflict（再読込を促す）、403 → forbidden、他 → error。再送はしない。
- `data/reducer.ts`: message の revision を下げない（遅れて届いた古い版を捨てる）。`message`・`message_revised` の event と `message_put` で、
  その発言を引く他の message の引用の状態を読み直す。`viewer` を `pending_ack` で知り、snapshot でも保つ。
- `data/revision.ts`（新規）: `revisableBy`（人 `human:` の取り次ぎでない発言で、表示している人が書いたものだけ。queued は編集だけ、
  completed・failed・interrupted は編集と撤回、running・cancelled・撤回済みは無し）、`citationStateNow`（retracted → edited → daemon の判定）、
  `restateMessageCitations`、409・403 の文。
- `messages/revision.tsx`（新規）: 「編集」「撤回」と入力欄、「編集済み（rev n）」「撤回済み（理由: …）」の 1 行、「前の版を見る」（GET …/revisions）、
  409 の「再読込」の button。
- `messages/citations.tsx`（新規）: 返事の案・要約の file ごとに、引いた発言の状態の印と、引いた発言への link（同じ thread は吹き出しへ移る、
  別の thread は `/?thread=…#chat-message-…`）。1 つでも current でない file に失効の警告。
- `messages/workspace-links.ts`: `fileCitations`（引用を path ごとにまとめ stale を付ける）、`chatMessageElementId`、`citedMessageHref`、状態の表示名。
- `messages/message-item.tsx`・`message-list.tsx`・`home/chat-home.tsx`: 吹き出しに id（移る先）、編集・撤回の操作・表示・前の版・引用の一覧を差し込む。
  操作の口（`revision`）を渡さない一覧には操作が出ない。

## 試験（提出時の HEAD で実行）

追加した vitest 28 件（`data/revision.test.ts` 14 件、`messages/revision.test.tsx` 14 件）。外部ネットワークには出ない（fetch は差し替え）。
既存の試験は変えていない。base では `data/revision.ts`・`messages/revision.tsx`・`messages/citations.tsx` が無いので 2 file とも読み込めない。
既存の試験では、編集・撤回の呼び出し・操作を出す発言・引用の状態はどれも検出できない（どの試験も触れていない）。

変異の確認: `revisableBy` の「表示している人と違う author なら出さない」と `citationStateNow` の「撤回なら retracted」を消し、
`citations.tsx` の失効の警告を常に出さないようにすると、7 件が落ちた（`Tests 7 failed | 21 passed (28)`）。元に戻して 150 件が通った。

| 試験 | 守る不変条件 |
|---|---|
| `chat_message_revision_client_edit_patches_with_expected_revision` | 編集は PATCH …/messages/{m} に text と expected_revision |
| `chat_message_revision_client_retract_posts_with_expected_revision_and_reason` | 撤回は POST …/retract に expected_revision と理由 |
| `chat_message_revision_client_revisions_reads_the_previous_bodies` | 前の版は GET …/revisions |
| `chat_message_revision_session_edit_sends_shown_revision_and_shows_the_answer` | 表示している revision を送り、応答をすぐ表示する |
| `chat_message_revision_session_retract_sends_reason_and_expected_revision_1_for_unchanged` | 変えていない発言の撤回は expected_revision 1 |
| `chat_message_revision_session_409_is_conflict_and_keeps_the_shown_message` | 409 は conflict で、表示を変えない |
| `chat_message_revision_session_403_is_forbidden` | 403 は forbidden |
| `chat_message_revision_own_human_message_offers_edit_and_retract` | 自分の発言に操作（queued は編集だけ、running・cancelled は無し） |
| `chat_message_revision_cos_relay_and_other_people_get_no_actions` | CoS・system・operator・MCP・取り次ぎ・他人・主体の無い発言・撤回済みに操作が無い |
| `chat_message_revision_viewer_is_learned_from_the_own_send_and_survives_snapshot` | 表示している人を自分の送信の確定で知り、snapshot で失わない |
| `chat_message_revision_own_message_shows_edit_and_retract` | 吹き出しに「編集」「撤回」の button |
| `chat_message_revision_cos_relay_and_other_people_show_no_actions` | CoS・取り次ぎ・他人の吹き出しに button が無い。操作の口が無い一覧にも無い |
| `chat_message_revision_list_hides_actions_on_other_people_once_viewer_is_known` | 一覧で自分の発言にだけ操作が出る |
| `chat_message_revision_edited_shows_rev_and_previous_versions_button` | 「編集済み（rev 3）」と「前の版を見る」 |
| `chat_message_revision_retracted_shows_note_and_no_actions` | 「撤回済み（理由: …）」、撤回済みに操作が無い |
| `chat_message_revision_unchanged_message_has_no_note_or_history` | 変えていない発言の markup に印・前の版が無い（回帰） |
| `chat_message_revision_previous_version_shows_the_old_body` | 前の版に古い本文・版・主体 |
| `chat_message_revision_conflict_asks_to_reload` | 409 の知らせに「再読込」、押すと再読込を呼ぶ |
| `chat_citation_message_revised_edit_marks_citing_reply_edited` | SSE の編集で引く側の状態が edited |
| `chat_citation_message_revised_retract_marks_citing_reply_retracted` | SSE の撤回で引く側の状態が retracted |
| `chat_citation_message_event_of_cited_message_restates_and_older_revision_is_dropped` | message event で読み直し、他 thread の引用は変えない、古い版で表示を戻さない |
| `chat_citation_state_now_follows_the_daemon_order` | retracted → edited → daemon の判定（missing を保つ） |
| `chat_citation_files_group_by_path_and_stale_when_any_is_not_current` | file ごとにまとめ、1 つでも current でなければ失効 |
| `chat_citation_all_current_has_no_stale_warning` | すべて current なら警告が無い |
| `chat_citation_edited_retracted_missing_show_state_and_stale_warning` | edited・retracted・missing の印と失効の警告 |
| `chat_citation_links_move_to_the_cited_message` | 同じ thread は `#chat-message-…`、別の thread は `/?thread=…#…`、吹き出しが同じ id を持つ |
| `chat_citation_reply_without_citations_is_unchanged` | 引用の無い返事の markup は変わらない（回帰） |
| `chat_citation_sse_retract_turns_the_shown_summary_stale` | 表示中の要約が SSE の撤回で失効の表示になる |

検査の結果は報告本文（result.json の summary と evidence）に書く。

## LoC（`git diff --numstat $CELERIS_WU_BASE` と新規 file の行数、web の TypeScript）

- production: +684 −13（変更 7 file で +231 −13、新規 3 file で 453 行: `revision.tsx` 265、`citations.tsx` 95、`data/revision.ts` 93）。
  純増の理由: 新しい UI（編集・撤回の操作と表示、前の版、引用の状態と失効の警告）で、置き換える旧い仕組みが無い。
- test: +483（新規 2 file）。

## 親の目的への寄与

- 確認済み（vitest）: 人は案件の CoS chat で自分の発言を編集・撤回でき、前の版を読める。CoS・取り次ぎ・他人の発言には操作が出ない（最終の権限は daemon の 403）。
  要約・案の添付ごとに、引いた発言が最新・編集された・撤回された・見つからないのどれかと、その発言への link が出る。
  1 つでも最新でなければ失効の警告が出て、表示中の画面でも SSE の編集・撤回で警告に変わる。改善 B（引用と revision 付きの共有）と observability に効く。
- 推定: 人が議論の要約・案を読むとき、元の発言が変わったことを見落としにくくなる。人が使って効くかは未確認。
- 未確認: 本物の browser での表示と、別の thread（統括）の吹き出しへ移る link の動き（e2e は release gate だけで走らせる）。
  表示している人（viewer）はこの画面での送信の確定で初めて分かるので、それまでは他人の人の発言にも操作が出る（押すと daemon が 403 にし、「書いた人だけ」と表示する）。

## 未解決事項

- viewer を送信前から知る口（例: health・session の主体）は API に無い。要るなら別の task で daemon に足す（提案）。
- `web/scripts/check-parity.mjs` が R30 の行で exit 1 になる（`agent-docs/web/feature-parity.md` の R30 が引く commit が HEAD の祖先でない）。
  この WU の差分に関係しない（その file は変えていない）。release gate で同じなら別の task で直す。

## 提案

- 表示している人の主体を thread の detail か session で返す（viewer を送信前に知り、他人の人の発言に操作を出さない）。
