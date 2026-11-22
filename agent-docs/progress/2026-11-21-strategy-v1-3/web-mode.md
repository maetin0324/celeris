---
title: 戦略層 v1 子 3 web-mode — 案件の CoS chat の composer に発言の扱い（議論・案・実行）の選択、message に扱いの印と主体の表示
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
---
# web-mode（WorkUnit。子 3 の段 web）

設計は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md) の D9 と、D7 のうち送信と表示の部分。
基準は `$CELERIS_WU_BASE`。既存の `web/features/chat` の composer・data・messages を延長した。独立した chat の実装は作っていない。
`crates/` と `web/api/generated` は変えていない（cite 段で再生成した型 `ChatDiscussionMode`・`ChatMessage.discussion_mode` をそのまま使う）。

## 実装前 brief

1. 根本原因: daemon は `discussion_mode` を受け付ける（mode-gate）が、web の送信は欄を送る手段が無く、message の扱いと主体（author・relay）も表示しない。
2. 責務 owner: 送信の本文は `data/session.ts` の `send`（欄の有無は `data/send.ts` の `discussionModeField` だけが決める）。選択は `composer/chat-composer.tsx`。
   表示は `messages/message-item.tsx` から呼ぶ `messages/speaker.tsx`。
3. 既存の再利用先: `session.send`・`sendWithRetry`・reducer の pending（再送）・`components/ui/select`・`components/ui/badge`・生成型。
4. 選択肢: (a) mode を thread の設定に持つ — ADR D1 の (a) と同じ理由で不採用。(b) composer の state に持ち、送信ごとに送る — 採用。
   主体の表示は (i) 本文から推定 — 不採用（ADR 2026-10-27 D1「本文からは読まない」）。(ii) `author`・`relay` の記録から導く — 採用。
5. 新しい状態・分岐・fallback・port: composer の React state 1 つ（選んだ扱い、既定は実行）と、pending の `discussion_mode`（再送で同じ扱いを送るため）。
   fallback・切替 flag は無い。
6. 撤去対象: `message-item.tsx` の局所の `roleName` を `speaker.tsx` に移した（主体の名前の正本を 1 つにする）。他に置き換える仕組みは無い。
7. 不変条件と試験: 下の「試験」。

## したこと

- `composer/chat-composer.tsx`: 案件 thread（`kind === "project"`）の composer だけ、送信の横に「発言の扱い」の select（議論・案・実行、既定は実行）を出す。
  議論・案を選ぶと、CoS が起票・変更をしないこと（API が拒否する）を 1 行で示す。統括・review・legacy の composer は変えていない。
  送信の要求は `composerSendInput` で組み、再試行の鍵（fingerprint）に扱いを含める（扱いを変えた送り直しは別の client_message_id）。
- `data/send.ts`: `DISCUSSION_MODE_LABEL`（選択肢と印の語の正本）と `discussionModeField`（discuss・propose だけ欄を返し、execute と未指定は `{}`）。
- `data/session.ts`: `send` の本文に `discussionModeField(input.discussion_mode)` を足し、pending に扱いを渡す。
- `data/reducer.ts`: `PendingMessage.discussion_mode`。同じ client_message_id の再送は最初の扱いを保つ。
- `home/chat-home.tsx`: 失敗した送信の「再送」で pending の扱いを送る（議論の発言が再送で実行に緩まない）。
- `messages/speaker.tsx`（新規）: `messageSpeaker`（D7 の表: `human:` → あなた、`operator:` → operator の発言・取り次ぎ、`mcp:` → MCP の発言・取り次ぎ、
  `cos:` の取り次ぎ → 統括 CoS が取り次いだ人の発言、返事 → CoS、system → システム、他の主体 → 記録の主体をそのまま）と、
  `MessageMeta`（人以外の主体の行と、議論・案の印）。`human:` でない主体の発言を「あなた」と表示しない。
- `messages/message-item.tsx`: 吹き出しの aria-label を `messageSpeaker` から取り、本文の上に `MessageMeta` を置く。送信中の発言にも扱いの印を出す。
  author が人で扱いが実行・未指定の発言の markup は変更前と同じ。
- `client.ts` は本文をそのまま JSON にして送るので変えていない（wire に欄が載ることは試験で確かめた）。

## 試験（提出時の HEAD で実行）

追加した vitest 12 件（`-t chat_discussion_mode` で 12 passed）。変更前の source（base の 7 file）に当てると、composer の 4 件と data の 4 件が
assertion・TypeError で落ち、`speaker.test.tsx` は `./speaker` が無く読み込めない（8 failed＋1 file 失敗）。

| 試験 | 守る不変条件 |
|---|---|
| `chat_discussion_mode_composer_project_thread_offers_discuss_propose_execute_with_execute_default` | 案件 thread に 議論・案・実行 の順の選択があり、既定は実行 |
| `chat_discussion_mode_composer_general_and_review_threads_are_unchanged` | 統括・review の composer に選択が無い |
| `chat_discussion_mode_composer_send_body_carries_the_chosen_mode` | 選んだ扱いが送信の要求に載る（queue・interrupt とも） |
| `chat_discussion_mode_composer_default_execute_sends_the_same_body_as_before` | 既定（実行・未指定）の要求は変更前と同じ key だけ（回帰） |
| `chat_discussion_mode_send_body_carries_discuss_and_propose` | session の本文に discuss・propose が載る |
| `chat_discussion_mode_send_default_execute_omits_the_field` | execute・未指定・null の本文は変更前と同じ 6 key（回帰） |
| `chat_discussion_mode_client_puts_the_field_on_the_wire_only_when_set` | fetch の JSON に欄が載るのは discuss のときだけ |
| `chat_discussion_mode_failed_send_keeps_the_mode_for_retry` | 失敗した送信の pending が扱いを保ち、再送で緩まない |
| `chat_discussion_mode_speaker_distinguishes_human_operator_mcp_and_cos` | D7 の主体を author・relay から区別する |
| `chat_discussion_mode_speaker_relayed_message_is_not_shown_as_the_person` | operator・MCP の発言を「あなた」と出さない |
| `chat_discussion_mode_mark_shows_discuss_and_propose` | 議論・案の印（取り次ぎ・送信中の発言にも） |
| `chat_discussion_mode_mark_absent_for_execute_and_unset` | 実行・未指定の発言の markup は変更前と同じ（回帰） |

既存の web の chat の試験（110 件）は変えずに通した。`composer/chat-composer.test.tsx` は import の行と末尾の describe を足しただけで、既存の主張は変えていない。

検査の結果は報告本文（result.json の summary と evidence）に書く。

## LoC（`git diff --numstat $CELERIS_WU_BASE`、web の TypeScript）

- production: +182 −15（7 file。うち新規 `speaker.tsx` 82 行）。純増の理由: 新しい UI（選択と印・主体の表示）で、置き換える旧い仕組みが無い。
- test: +307 −1（新規 2 file と composer の試験）。

## 親の目的への寄与

- 確認済み: 人は案件の CoS chat で、発言ごとに議論・案・実行を選んで送れる。選んだ扱いは daemon の `discussion_mode` の欄に載り（試験）、
  mode-gate の API の拒否（ADR D2）が効く入力になる。発言の主体（人・operator・MCP・統括 CoS の取り次ぎ）が画面で分かれ、取り次ぎを人の発言と取り違えない（試験）。
- 推定: observability（人が議論の記録を読むとき、誰の発言で、議論として送ったかが分かる）に効く。人が使って効くかは未確認。
- 未確認: 本物の browser での表示（e2e は release gate だけで走らせる）。select の電話幅での並び。

## 未解決事項

- 編集・撤回の操作、引用の状態の表示は並行の WU web-cite の範囲（この WU では触れていない）。
- 選んだ扱いは composer の state にだけあり、thread を開き直すと既定の実行に戻る（ADR D1 の選択 (a) を採らない決定のとおり）。

## 提案

- なし。
