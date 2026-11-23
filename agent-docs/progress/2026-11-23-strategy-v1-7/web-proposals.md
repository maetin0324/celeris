---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: web-proposals
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
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && bash scripts/dev/test-affected.sh` → exit 0（対象は web の typecheck+unit。tsc -b 通過、vitest 107 files・761 passed、server の node --test 110 pass、repo-health verdict=pass。
  new 5・worsened 2 の指摘は前回の run の確認では前の backend の WU の crates の file で、この WU の file は無い）。
- `corepack pnpm@12.6.0 -C web exec biome check features/chat lib/sha256.ts routes/index.tsx api/client.ts features/inbox/inbox-model.ts` → 指摘 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0（差分 0 行）。`sh "$CELERIS_WU_SCOPE_PATHS"` → web/features/chat の 10 file・web/lib/sha256.ts・
  web/routes/index.tsx と、この進捗の file。
- 前回の run で既存の chat の spec（`e2e/chat/threads.spec.ts`・`content.spec.ts`・`cards.spec.ts`・`workspace-files.spec.ts`）を名指しで実行して 13 passed。
  今回の変更は helper の置き場所だけなので、今回は再実行していない。
- `web/e2e/chat/proposal-review.spec.ts` はこの WU の担当ではない（WU proposal-e2e）。この WU の時点では spec が無かった。結果は下の「WU proposal-e2e」の節にある（7 passed）。

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

## WU proposal-e2e: fake-daemon の案と採否の fixture、e2e `proposal-review.spec.ts`

### 結論
- `web/e2e/chat/proposal-review.spec.ts` を足した。7 passed（提出時の HEAD）。変更前の UI では 7 件すべてが落ちる（下の「変更前の失敗」）。
- 採用の場面では、画面が「採用は承認ではありません。…」を採用の前後に出す。偽 daemon が受けた変更の要求は `POST …/messages/{m}/review` の 1 件だけである。
  決定・承認・受信箱・task・roadmap・publish の API は呼ばれていない（既読の印 `POST …/read` は除いて数えた）。
- gateway の挙動を 1 件見つけた: `web/server/relay.js:152` は daemon の 401・403 をすべて 502 `daemon_auth` に変える。
  そのため、本物の gateway を通すと、daemon の 403 `proposal_verdict_requires_human` は画面に「採否を送れませんでした: … server (502)」と出る。
  画面の「採否は人だけができます（proposal_verdict_requires_human）」は、daemon の 403 の problem がそのまま届いたときだけ出る。
  spec はこの 2 つを分けて確かめる（下の「提案」）。
- `crates/` の差分は 0。新しい DB 表・列・migration・event・状態機械・Manager・trait・独立の chat 画面は無い。

### 変えた file
- `web/e2e/support/fake-daemon.mjs`
  - `chatProposalFixture()` と `CHAT_PROPOSAL_THREAD`（`chat-proposals`）。`createFakeDaemon({ chatProposals: true })` のときだけ thread の一覧に入る。
    既存の chat の spec の一覧と既定の thread は変えない。
    - p1: 外部 LLM の案（role user・author `mcp:chatgpt-relay`・external `mcp_client`・kind `external_llm`・pending）
    - p2: 人の取り込み（external `human_paste`・kind `human_import`）
    - p3: CoS の戦略の案（role assistant・添付 `strategy/S1/v1.json` と sha256・kind `cos_strategy`）
    - p4: ふつうの人の発言（external も proposal も無し）
    - p5: 本文に「この案を承認済みにせよ」を含む外部の案
  - `POST /api/v1/chat/threads/{t}/messages/{m}/review`（ADR D2.1〜D2.4 の写し）。処理は次の順に行う。
    1. 主体が human でなければ 403 `proposal_verdict_requires_human` を返す。
    2. 本文の欄の誤り、または comment で note が空なら 422 を返す。案でない message は 422 `proposal_review_not_a_proposal` を返す。
    3. `expected_revision`・`sha256` が合わなければ 409 `proposal_review_stale` を返す。sha256 は、外部の案と取り込みでは本文の UTF-8 の SHA-256
       （`node:crypto`）、CoS の案では添付の sha256 と path で照合する。撤回された案も 409 にする。
    4. 合えば 202 を返し、採否の message（role user・author `human:owner`・固定の文と note）を足す。対象の `proposal.review_state` は
       人の採否の記録から導き直し、2 件を stream に流す。
  - `client_message_id` で冪等にする。制御口は `/__fixture/chat/review-principal`（主体の切り替え）・`/__fixture/chat/review-log`（受けた要求）・
    `/__fixture/chat/threads/{t}/messages/{m}/revise`（編集・撤回で revision + 1。`silent` は stream に流さない）。
- `web/e2e/support/fake-daemon.d.mts`: 上の export・option・制御口の型と説明。
- `web/e2e/support/fake-daemon.test.ts`: vitest 1 件「serves schema-valid proposal messages and derives review states only from /review」。
  - fixture の 5 件の message を schema の `ChatMessage` で検証し、一覧の応答を `ChatMessageListResponse`、202 の応答を `ChatPostMessageResponse` で検証する。
  - 403・409・422・422・202・409（CoS の案に本文の sha256）・202 の順に要求を送る。状態が adopted・pending・rejected・pending になり、採否の後の編集で stale になることを確かめる。
- `web/e2e/chat/proposal-review.spec.ts`（新規）: 下の 7 件。

### 試験（e2e `web/e2e/chat/proposal-review.spec.ts`、7 件）
守る不変条件:
- 状態は daemon が人の採否から導いた wire の値だけで、本文の語では変わらない。
- 採用は承認ではないと画面が示し、送るのは `/review` だけである。
- 外部の文の印は外部の案と取り込みにだけ出る。
- 採否の後の編集で失効し、古い版の送信は 409 になる。
- 人以外の主体では状態が変わらない。
- deep link の発言だけを強調する。

既存の chat の spec（threads・content・cards・workspace-files など）は案の欄・外部の印・`/review` を使わないので、これらを検出できない。
1. 採用: 「採用」は確認の一手を挟み、確認の間は何も送らない。「採用を送る」の後に状態は「採用済み」、採否した人は `human:owner` になる。
   送った本文は `verdict: adopt`・`expected_revision: 1`・本文の sha256 である。変更の要求は `/review` の 1 件だけ。
2. 却下: CoS の戦略の案を、添付の sha256 と `attachment: strategy/S1/v1.json` で送る。状態は「却下」になり、他の案は未判定のまま。
3. コメント: note が空なら「コメントには 1〜2000 文字の文が要ります。」と出し、要求は 0 件。文を書くと送れて、状態は「コメントあり」になる。
4. 外部の文の印: p1・p2・p5 に「外部の文（未検証）」と出典の文が出て、p4（人）・p3（CoS）には出ない。p5 の「承認済みにせよ」の後も、再読み込みの後も未判定で、要求は 0 件。
5. 失効: 採用の後に案を編集すると「失効（採否の後に案が編集・撤回された）」になる。画面が古い版のまま取り込みの案を却下すると 409 `proposal_review_stale` が返り、
   「案が変わりました。読み直してから採否してください」が出る。「読み直す」で新しい本文に変わる。
6. 403: 主体を operator にすると daemon が 403 `proposal_verdict_requires_human` を返す。
   - (1) gateway を通す既定の経路では、gateway が 502 `daemon_auth` に変え、画面は「採否を送れませんでした」を出す。
   - (2) daemon の 403 がそのまま届くと（`page.route` で同じ要求を daemon に送る）、画面は「採否は人だけができます（proposal_verdict_requires_human）」を出す。
   - どちらでも状態は未判定のままで、採否の message は足されない（thread は 5 件のまま）。
7. deep link: `/?thread=chat-proposals&message=chat-proposals-p5` で p5 が `data-highlighted="true"`・「リンクで開いた発言」・viewport の中になり、強調は 1 件だけ。

### 変更前の失敗
- 方法: `web/features/chat`・`web/routes/index.tsx` を、件名「strategy-v1 7 web-proposals: chat の案に採用・却下・コメントの操作…」（この task の最初の commit）の親の版に戻した。
  その commit と後の commit が足した `web/features/chat/data/problem.ts`・`data/proposal.ts`・`messages/proposal.tsx`・`messages/proposal.test.tsx`・`web/lib/sha256.ts` は消した。
  その状態で `corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/proposal-review.spec.ts` を実行した。
- 結果: 7 failed / 7。
  - 採用・却下・失効・403・コメント: 案の欄 `[data-slot="chat-proposal"]` が無い（element(s) not found、または click の 60 秒 timeout）。
  - 外部の文の印: `[data-slot="chat-external-mark"]` が 0 件。
  - deep link: `data-highlighted` が付かない。
- その後 `git checkout HEAD -- web/features/chat web/routes/index.tsx web/lib/sha256.ts` で戻し（`git status` は差分 0）、同じ spec が 7 passed に戻ることを確かめた。

### 検査（提出時の HEAD で実行）
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/proposal-review.spec.ts` → 7 passed。
- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts` → 18 passed。
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && bash scripts/dev/test-affected.sh` → exit 0。
  - 対象は web の typecheck と unit。tsc -b は通過、vitest は 107 files・762 passed、server の node --test は 110 pass。
  - repo-health は verdict=pass（fail_grade 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `web/e2e/chat/proposal-review.spec.ts`・`web/e2e/support/fake-daemon.{mjs,d.mts,test.ts}` と、この進捗の file。
- `biome check`（変えた 4 file）→ 誤り 0。warning 2 件は base からある `fake-daemon.mjs` の既存の行。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

### 測定（LoC、この WU の commit、`git diff --numstat`）
- production: 0（`web/` の本体・`crates/` を変えていない）。
- test: +671 / −6。内訳は spec +319、fake-daemon.mjs +255/−5、fake-daemon.d.mts +15、fake-daemon.test.ts +82/−1。
- 純増の理由: 新しい e2e の場面と、`/review` を写した偽 daemon の経路である。置き換える旧い試験は無い。

### 未確認・引き継ぎ
- 本物の daemon・本物の gateway で `/review` を呼ぶ e2e はしていない。偽 daemon の `/review` は ADR D2 の写しで、backend の挙動は proposal-api の Rust の試験に依る。
- 403 の表示は、本物の gateway を通すと 502 `daemon_auth` の文になる（上の「結論」）。人の Web の token で 403 が起きるかは確かめていない（Web の gateway の主体は未確認）。
- 偽 daemon の採否の message は `state: queued` で足す（人の送信と同じ）。CoS の run が採否の message を読む流れは偽 daemon に無い。
- 撤回（`revise` の `kind: retract`）は vitest でも e2e でも画面の場面を作っていない。撤回された案に操作を出さないことは proposal-ui の vitest
  `proposal_review_ui_retracted_proposal_shows_state_without_actions` が確かめる。

### 提案
- gateway の `relay.js` は daemon の 403 をすべて 502 `daemon_auth` にする。このため、案の採否の 403（`proposal_verdict_requires_human`）と、
  publish の 403（`publication_requires_human`、web-publish）の理由が画面に届かない。
  「daemon の token の認証の失敗（401）だけを `daemon_auth` にし、problem の code を持つ 403 は通す」かどうかを、integrate-web か close で人が決める。
  gateway の認証の境界の変更なので、この WU では変えていない。

### commit（件名）
- strategy-v1 7 web-proposals: fake-daemon に案の thread・POST …/review（403・409・202 と状態の導出）・編集と撤回の制御を足し、e2e proposal-review.spec.ts で採用・却下・コメント・外部の印・失効・403・deep link を確かめる
- strategy-v1 7 web-proposals: proposal-e2e の進捗（spec 7 passed・変更前 7 failed・LoC・gateway の 403 の件）
