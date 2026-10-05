---
title: Web work 画面群 visual QA — ホーム・受信箱・通知の修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# ホーム・受信箱・通知の visual QA 修正

`qa-work.md` の critique を、コード・操作試験・撮影で 1 往復確認した。コード commit は `9abbfd92`。pre は `wu/critique/artifacts/qa-qa-work-pre/`、post は `wu/fix-inbox/artifacts/qa-work-fix-inbox-post/`（通常 12 枚）と `qa-work-fix-inbox-post-states/`（状態 116 枚）。いずれも 360/390/412/1440px。以下の「残課題」は、この leaf の許可範囲と parity の期待を守るために残した箇所を示す。

| ID | 結果 |
|---|---|
| W-01 高 | **修正** `9abbfd92`: 各判断の見出し直下に問いを置き、回答・推奨・期限・案件の順にした。止めている範囲と関連は補助の折りたたみに移した。post `_inbox-360.png` で最初の選択肢まで見える。 |
| W-02 高 | **残課題**: 推奨以外の非破壊選択（例「計画をやり直す」）は即送信のまま。`web/e2e/parity/inbox.spec.ts` がそのボタンの直後に API 応答と行内エラーを要求しており、確認を挟むと「parity の期待を書き換えない」条件に反する。取り下げなど既存の破壊キーは ConfirmDialog のまま。API に取消操作もない。parity と確認手順を同時に更新できる作業で直す。 |
| W-03 高 | **修正** `9abbfd92`: ホームに期限順の判断待ち 3 件を、題名・期限・案件名とともに会話より上へ表示。通知は件数の入口にした。期限 24 時間以内は注意 badge。 |
| W-04 高 | **修正** `9abbfd92`: SSE 再接続・切断または判断待ち取得失敗の間、ホームに鮮度の注意と「未確認」を表示。**残課題**: shell のスマホ幅の最終受信時刻は `web/components/shell/` の範囲外。 |
| W-13 中 | **修正** `9abbfd92`: summary があるとき units の重複を出さず、コードで作る表示を「作業単位」に変更。**残課題**: API から来る summary・effect 内の「葉」「planner」は原文で、parity がその表示を期待する。API 文言または parity の協調変更が必要。 |
| W-14 中 | **修正** `9abbfd92`: 11 件以上は期限順の一覧にし、回答欄を同時に 1 件だけ開く。開閉ボタンは Enter でも動き、focus を保持することを新しい e2e で確認。post `many-_inbox-360.png` で textarea の壁が消えた。 |
| W-15 中 | **修正** `9abbfd92`: 推奨・期限・案件を desktop で 2 列に配置し、長い関連リンクは補助節へ移した。リンク自体の 44px target は維持。 |
| W-16 中 | **修正** `9abbfd92`: 取得済みの案件名を通知に使い、未取得の長 ID は ShortId にした。**残課題**: task リンクの「タスク」「task T3」は `web/e2e/parity/notifications.spec.ts` が accessible name を固定している。Notice 型には task title も無い。 |
| W-17 中 | **修正** `9abbfd92`: 受信箱の可視ラベルを「理由（メモ）」、種類を「下書きの受け入れ」にした。**残課題**: parity が `理由・note` の accessible name とエラー文、通知の `task の完了`、ホーム会話の `run の作業`・`Console への入力` を固定する。会話の実装は担当範囲外。 |
| W-18 中 | **確認**: `secondary` は既に `border-input` を持つ。新しい e2e で未読時にボタンが enabled かつ computed border が 1px と確認した。pre 撮影の薄い見た目は取得途中の可能性が高い。未読 0 件では近接する「未読はありません」が無効理由になる。 |
| W-19 中 | **修正** `9abbfd92`: FetchFrame に subject「判断待ち」を渡し、403 時に管理者へ権限確認を案内する。**残課題**: 「一覧へ戻る」の実際の行き先と `<a>` は共通の `fetch-frame.tsx` にあり、この leaf で変更できない。必要な権限名も API から得られない。 |
| W-20 中 | **修正** `9abbfd92`: 判断待ちの行に沿う 2 行の skeleton と subject を設定。**残課題**: 「再取得」とボタン「再試行」の不一致は共通 FetchFrame の範囲外。 |
| W-44 低 | **修正** `9abbfd92`: 判断待ちを warning、失敗を danger に分け、期限 24 時間以内を「まもなく期限」とした。 |
| W-45 低 | **残課題**: 通知の各行にある既読ボタン位置と多数件時の操作密度は維持。通知を開く動線の意味と既読化の失敗表示をそろえる変更が先に必要。 |
| W-46 低 | **残課題**: リンクからの自動既読失敗はまだ黙殺される。遷移前後で結果を伝える共通の操作結果の持ち越しが要る。 |
| W-47 低 | **残課題**: Console 入力欄と会話の語・余白は `web/features/console/` の担当範囲外で、parity も accessible name を固定している。ホームの空きには判断待ちを置いた。 |

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 最初は tarball 不足で失敗。依存を補充後に同じ offline コマンドで exit 0。
- `typecheck`・`lint`・`build`・`test`・`check:boundaries`・`check:parity`: exit 0。lint は担当外の既存警告 5 件。
- `playwright test e2e/parity/inbox.spec.ts e2e/parity/notifications.spec.ts e2e/work/inbox-notifications.spec.ts`: 14 passed / 2 screenshot tests skipped、exit 0。
- `mobile-audit --only /`・`--only /inbox`・`--only /notifications`: 各 4 幅で exit 0。
- 全画面の `mobile-audit` は exit 1。違反は担当外の `/projects/P1` の textarea 名、`/tasks/T1` のリンク target と textarea 名、`/tasks/T1/changes` の textarea 名。対応する画面と監査 script はこの leaf の許可範囲外。担当画面の違反は 0 件。

## 再評価

post `_inbox-360.png` と `many-_inbox-360.png` で問いと最初の操作が早く読める。通常時の判断待ちは通知の上位にあり、未確認状態は注意で示す。40 件状態で常設フォームがなくなり、操作の focus 順も短い。スクリーンショット台本の `fullPage` は依然として 800px で止まるため、折り返しと長 ID の画面外部分は e2e と mobile-audit で確認した。高優先の未解決は W-02 の確認手順で、parity 更新を伴う別作業が必要。
