---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: proposal-ui
status: done
completed: 2026-11-23
---
# 戦略層 v1 子 7 web-proposals（WU proposal-ui）: chat の案に採用・却下・コメントの操作と外部の文の印

## 結論
- chat の吹き出しのうち wire に `message.proposal` がある案（外部 LLM の案・人の取り込み・CoS の戦略の案）に「案の採否」の欄を足した。
  欄は種類・状態（未判定・採用済み・却下・コメントあり・失効（採否の後に案が編集・撤回された））・採否した人と時刻・採否の発言への link と、
  採用・却下・コメントの操作を出す。撤回された案には操作を出さず「撤回された案は採否できません。」と出す。
- 採用の操作の下に常に「採用は承認ではありません。CoS に戦略の版（strategy/<id>/v<n>.json）と人の承認（決定 strategy-approve-v<n>）へ
  進めるよう指示します。」を出す。採用は「採用を送る」の確認の一手を挟む。却下・コメントは押すとすぐ送る。
- 送る本文（ADR 2026-11-23-project-observability-publish D2.1）: `expected_revision` は `message.revision`（省略時 1）。`sha256` は
  `cos_strategy` なら `workspace_files` のうち `strategy/<id>/v<n>.json`（`task_core::strategy_ref::strategy_artifact` と同じ形）の添付の sha256 で、
  `attachment` にその path を入れる。それ以外は本文 `message.text` の UTF-8 の SHA-256 の小文字 16 進（`body_sha256` と同じ値）。
  `client_message_id` は `web/lib/random-id.ts` で毎回作る。note は空白だけなら送らず、コメントは 1〜2000 文字（code point）を画面で先に確かめる。
- 外部の文の印: wire に `message.external` がある吹き出しに「外部の文（未検証）」の badge と「出典: <source_label>（<source_kind>）」を文字で出す。
  wire が `external` を付けない人の発言には出ない。
- 状態は wire の値だけを表示する。送った後は既存の再取得（`session.refresh()`、thread と messages の snapshot の取り直し）で読み直し、
  画面は採否の正本を持たない。本文に「承認済み」「採用済み」「approve」があっても状態は変わらない。
- 失敗の表示: 403 は「採否は人だけができます（<code>）」（code が無ければ `proposal_verdict_requires_human`）、409 `proposal_review_stale` は
  「案が変わりました。読み直してから採否してください」と「読み直す」の button、422 と他の 409 は problem の detail。
- deep link: `web/routes/index.tsx` の search に `message` を足した。`/?thread=<t>&message=<m>` で、読み込み済みの message へ
  `scrollIntoView({block:"center",behavior:"smooth"})` と focus で移り（citations.tsx と同じ方法）、枠と「リンクで開いた発言」の文字で強調する（1 度だけ）。
- `crates/` の差分は 0。新しい DB 表・列・migration・event・状態機械・Manager・trait・独立の chat 画面は無い。

## 変えた file
- `web/lib/sha256.ts`（新規）: `sha256HexOfBytes`・`sha256Hex`（FIPS 180-4 の純粋な TS。`crypto.subtle` は http の origin に無い）。
- `web/features/chat/data/client.ts`: `chatPaths.messageReview` と `reviewProposal(threadId, messageId, body)`（`apiMutate` の POST、202 は `ChatPostMessageResponse`）。
- `web/features/chat/data/proposal.ts`（新規）: 状態・種類・verdict の語、採用の注意文、`isStrategyArtifactPath`・`strategyDraftOf`・`proposalActionable`・
  `proposalNoteError`・`proposalReviewRequest`（送る本文）・`proposalReviewFailure`（403・409・422 の文）。
- `web/features/chat/messages/proposal.tsx`（新規）: `ProposalReview`（欄と操作）・`ExternalMark`（外部の印）・`ProposalReviewActions`（submit・reload の口）。
- `web/features/chat/messages/message-item.tsx`: 外部の印と採否の欄を描く。`highlighted` の強調。
- `web/features/chat/messages/message-list.tsx`: `proposalReview` と `focusMessageId` を渡し、deep link の scroll と focus。
- `web/features/chat/home/chat-home.tsx`: search の `message` を読み、`reviewProposal` と `session.refresh` で口を組む。
- `web/routes/index.tsx`: search に `message`。
- `web/features/chat/data/problem.ts`（新規）・`web/features/chat/cards/card-model.ts`: card-model の中にあった `problemCode`・`problemDetail` を
  chat の data 層に移し、card-model と `data/proposal.ts` の両方がそこから使う（chat の中で 2 つ目の写しを作らない。規約 2）。card-model の挙動は変えていない。
  `web/features/inbox/inbox-model.ts` には base から同じ形の写しがあるが、許可 path の外なので変えていない（統合は別の task の候補）。
- e2e の hook（WU proposal-e2e 向け）: `data-slot` の `chat-proposal`（`data-review-state`・`data-proposal-kind`）・`chat-proposal-state`・`chat-proposal-reviewed`・
  `chat-proposal-note`・`chat-proposal-adopt`・`chat-proposal-reject`・`chat-proposal-comment`・`chat-proposal-adopt-notice`・`chat-proposal-confirm`（button「採用を送る」「やめる」）・
  `chat-proposal-error`・`chat-proposal-sent`・`chat-proposal-retracted`・`chat-external-mark`（`data-untrusted="true"`）・`chat-external-source`・`chat-message-linked`、
  吹き出しの `data-highlighted="true"`。button の文字は「採用」「却下」「コメント」。

## 試験
守る不変条件: 状態は wire の `proposal.review_state` だけから出す（本文の語で変わらない）・採用は承認ではないと常に書く・採用は確認の一手を挟む・
外部の印は wire の `external` があるときだけ出す・撤回された案に操作を出さない・送る sha256 は `body_sha256` と同じ値で、CoS の案は添付の sha256 と path・
403・409・422 を決めた文で出す・deep link の発言だけを強調する。base には案の欄も外部の印も `reviewProposal` も `web/lib/sha256.ts` も無いので、
既存の試験（messages・revision・citations）はこれらを描かず検出できない。下の試験は変更前に全部落ちる（import 先の file・export が無い）。
- vitest（`web/features/chat/messages/proposal.test.tsx`、10 件）:
  `proposal_review_ui_sha256_matches_known_values_and_utf8_japanese_bodies`（NIST の "" と "abc"、56 byte の 2 block 目、日本語・絵文字と改行・300 byte の本文。
  値は `printf '%s' … | sha256sum` で出した）・`proposal_review_ui_shows_the_five_state_words_from_the_wire`・
  `proposal_review_ui_always_shows_the_adopt_is_not_approval_notice_and_three_actions`・`proposal_review_ui_external_mark_only_on_wire_external_messages`・
  `proposal_review_ui_state_stays_the_wire_value_whatever_the_body_says`・`proposal_review_ui_request_body_carries_the_seen_revision_and_sha256`・
  `proposal_review_ui_retracted_proposal_shows_state_without_actions`・`proposal_review_ui_maps_403_409_422_problems_to_text`・
  `proposal_review_ui_client_posts_the_body_to_the_review_route`・`proposal_review_ui_deep_link_highlights_the_loaded_message`。
- 確認の一手（「採用」を押すと「採用を送る」が出て、押すまで送らない）・送信後の読み直し・失効と 403 の画面の表示は、vitest が node 環境の静的描画なので
  click を試せない。WU proposal-e2e の `web/e2e/chat/proposal-review.spec.ts` が確かめる。

## 検査（提出時の HEAD で実行）
- 前回の run の範囲 check の不合格（`web/api/client.ts`・`web/features/inbox/inbox-model.ts` が許可 path の外）を直した: 2 file を `$CELERIS_WU_BASE` の
  内容に戻し、helper を `web/features/chat/data/problem.ts` に置いた。同じ範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path から許可 path を除く）→ 出力 0 行、exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/chat/messages/proposal.test.tsx` → `proposal_review_ui_` 10 passed。
- `corepack pnpm@12.6.0 -C web exec vitest run features/chat features/inbox lib` → 190 passed。
- `corepack pnpm@12.6.0 -C web exec tsc -b` → exit 0。
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && bash scripts/dev/test-affected.sh` → TEST_AFFECTED_RESULT
- `corepack pnpm@12.6.0 -C web exec biome check features/chat lib/sha256.ts routes/index.tsx api/client.ts features/inbox/inbox-model.ts` → 指摘 0。
- `cargo clippy --workspace -- -D warnings` → CLIPPY_RESULT。`cargo fmt --all -- --check` → FMT_RESULT。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0（差分 0 行）。`sh "$CELERIS_WU_SCOPE_PATHS"` → web/features/chat の 10 file・web/lib/sha256.ts・
  web/routes/index.tsx と、この進捗の file。
- 前回の run で既存の chat の spec（`e2e/chat/threads.spec.ts`・`content.spec.ts`・`cards.spec.ts`・`workspace-files.spec.ts`）を名指しで実行して 13 passed。
  今回の変更は helper の置き場所だけなので、今回は再実行していない。
- `web/e2e/chat/proposal-review.spec.ts` はこの WU の担当ではない（WU proposal-e2e）。この WU の時点では spec が無いので結果は未確認。

## 測定（LoC、基準 `$CELERIS_WU_BASE`、`git diff --numstat`）
- production（web）: +536 / −15（sha256 +74、data/proposal +130、data/problem +16（新規）、messages/proposal +233、client +11、message-item +20、message-list +25、
  chat-home +23/−2、routes +3/−1、card-model +1/−12）。
- test: +345（vitest 1 file）。
- 純増の理由: 新しい操作の欄と SHA-256 の実装で、置き換える旧い仕組みが無い（ADR D4）。problem の helper は card-model から chat の data 層へ移しただけで、写しは増やしていない。

## commit（件名）
- strategy-v1 7 web-proposals: chat の案に採用・却下・コメントの操作・状態 5 語・採用の注意、外部の文の印、/?thread=&message= の強調、SHA-256 の純 TS 実装と proposal_review_ui_ の試験
- strategy-v1 7 web-proposals: 進捗を書く
- strategy-v1 7 web-proposals: problem の helper を chat の data 層に移し、範囲の外の api/client.ts・inbox-model.ts を base に戻す

## 未確認・引き継ぎ
- `cos_strategy` の案が `strategy/<id>/v<n>.json` を 2 つ以上添えたときは、`workspace_files` の最初の 1 件で採否する。daemon の状態の導出が
  どの添付を照合するかは確かめていない（ADR D2.2 は添付 1 件を前提に書く）。
- `cos_strategy` の添付の sha256 は `useAttachmentMap` が読んだ添付の metadata から取る。読み終える前に押すと「戦略の版の添付を読み込めていません」と出して送らない。
- deep link は読み込み済みの message だけに移る。古い頁にある message は「以前のメッセージ」で読み込んでも自動では移らない（search を開き直すと移る）。
- web-overview の未確認の「chat の link は `/?thread=` だけ」は、この WU で search に `message` が入ったので、overview の chat の link に `message` を足すかを
  integrate-web か close で決める（この WU は projects の画面を変えていない）。
- 本物の daemon・本物の browser での確認はしていない（e2e は fake-daemon）。

## 親の目的への寄与
- observability と publish の責任（確認済み、unit 試験で。画面の操作は proposal-e2e の e2e で確かめる予定）: 人は ChatGPT などから送られた案を
  既存の chat の画面で採用・却下・コメントでき、外部の文を人の発言と文字で見分けられる。採用が承認ではないことを画面が常に示す。
- 改善 A〜D への直接の寄与は無い（採否の記録と状態の導出は proposal-api の backend の試験に依る）。
