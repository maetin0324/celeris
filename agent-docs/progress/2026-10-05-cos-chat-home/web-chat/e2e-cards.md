---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---
# web-chat / e2e-cards（その場回答・代答の修正・受信箱件数）

ADR 2026-10-05-cos-chat-home D6 UI 行のカード部分を `web/e2e/chat/cards.spec.ts` の 6 試験で検証した。fake-daemon の制御 endpoint と `expect.poll`・DOM/API の出来事待ちを使い、sleep は使わない。

## 検証内容

- task・決定・質問・認可・plan gate・知らせ・CoS 代答の 7 種類の表示、状態と推奨選択肢。
- 決定・質問・認可・plan gate のその場回答。対応する受信箱項目が消え、カードに回答結果が残る。
- 理由必須の選択肢は詳細へ誘導する。通常の plan gate の取り下げもこの経路。理由不要の破壊的選択肢を返す専用 fixture では確認ダイアログを挟み、戻るでは項目が残り、確定で消える。
- CoS 代答の理由入力必須、差し戻しの 409 競合表示、競合後の取消成功、差し戻し成功。API の action・reason・operation_id とカードの結果表示を検証し、成功後の再操作ボタンが消えることも確認。
- task 詳細への SPA 遷移、履歴で元の会話へ戻る、代答詳細の URL。
- 受信箱 thread の badge は全受信箱の人待ち項目数（6 件）を表示する。表示中のカード以外の項目も含み、回答後は 5 件に減る。

## 実装と修正

`features/chat/home/chat-home.tsx` が既存の `inboxItemsQuery()` を購読し、`counts.total` を `ChatThreads.inboxWaitingCount` に渡す。前 run の threads 内の query cache 購読・listMessages 再読み込みは撤回した。受信箱 query は shell と同じ key を共有する。

fake-daemon に override の成功・409 切替と要求履歴を追加した。カードの id は実在の受信箱項目に対応し、operation は操作可能な applied 状態で seed する。質問を加える `chatInboxItemsFixture()` はチャット専用とし、共用の受信箱 fixture の 5 件と題名を保つ。

前 run の代答試験の履歴ヘルパーは GET 専用 endpoint を POST で呼んでいたため GET に修正。今回の初回カード試験は 4 passed / 1 failed（取り下げるが理由必須でボタンにならない）だった。仕様を変えず、理由不要の確認経路を専用 fixture に分けて修正した。debug spec・fixme は残していない。

## 証拠コマンドと結果

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 最初は cache 不足（path-to-regexp）で exit 1。`install --frozen-lockfile` で依存を取得した後、最終の offline install は exit 0。lockfile 変更なし。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web lint`: exit 0（既存 styles.css の !important warning 4 件）。
- `corepack pnpm@12.6.0 -C web test`: exit 0（vitest 70 files / 485 tests、node server 57 tests。fake-daemon 10 件を含む）。
- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts`: exit 0、10 passed。最終変更も上記 web test に含む。
- `corepack pnpm@12.6.0 -C web build`: exit 0。home の UI 変更後に生成し、その後の fixture/spec 修正では dist を再利用。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/cards.spec.ts`: exit 0、6 passed。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: 最終 exit 0、20 passed（cards 6 + 既存 14）。
- `git diff --check`: exit 0。
- `git diff --name-only "$CELERIS_WU_BASE"` と untracked の一覧を `web/e2e/chat/`・`web/e2e/support/`・`web/features/chat/`・本進捗ファイルの範囲で検査: 範囲外 0 件。gui/・crates/ の差分 0 件。

## 未解決

この WorkUnit の受け入れ条件について未解決なし。実 daemon による代答の副作用・監査は本試験の対象外であり、fake の API/UI 成功のみを確認した。

## 提案

後続の e2e-attach・e2e-narrow で添付・狭い幅・mobile-audit・screenshot を検証する。前 run の添付用ヘルパー追加は本差分から除いたので、必要なものはその担当葉で追加する。
