---
title: qa-work fix-reports — 報告・承認の critique 指摘の修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# qa-work fix-reports — 報告・承認の critique 指摘の修正

親の記録: [qa-work.md](../qa-work.md) の『## critique』の fix-reports 担当（W-10〜12・W-31〜36・W-48〜50）。
編集した file: `web/features/approvals/approvals-screen.tsx`・`web/features/reports/reports-screen.tsx`・新規 `web/e2e/work/reports-approvals.spec.ts`。route file（`web/routes/reports.tsx`・`approvals.tsx`）は変更不要。
修正の commit: `bb8ce583`（報告・承認の画面で出所と結果を読めるようにし、常設ルールの誤追加を防ぐ）。

## 修正

| ID | 重さ | 対応 | commit |
|---|---|---|---|
| W-10 | 高 | **一部（残課題あり）**。追加の確認ダイアログは置かない（下の残課題）。代わりに送信前に「追加すると、{課}の認可の依頼のうちこの規則に一致するものは、確認なしで自動で認められます。取り消すには一覧の『削除』」を常に表示し、追加直後に「いま足した規則を取り消す」（作成応答の id を DELETE）を出す。 | bb8ce583 |
| W-11 | 高 | 直した。常設ルールの一覧が loading・error・disconnected・403 の間は追加フォームの fieldset を disabled にし、Notice「既存のルールを確認できないため、追加を止めています。」（403 は「権限がありません」）を出す。 | bb8ce583 |
| W-12 | 高 | 直した。結果ごとに Badge の色を分けた（今回だけ success・今後も warning・認めなかった danger・取り下げ neutral）。「今後も認めた」には「常設ルールを見る」（`#standing-rules`）を添える。 | bb8ce583 |
| W-31 | 中 | 直した。履歴の行に 依頼元（組織の課名、不明なら ShortId）・「元のタスクを開く」（`/tasks/$id`）・決めた日時・回答 を出す。並びは新しく決めた順（`decided_at` 降順、無ければ `created_at`）。 | bb8ce583 |
| W-32 | 中 | 直した。種類を和名（悪い知らせ・質問・提案・結果・進捗）にし、悪い知らせ danger・質問 warning・提案 info。段は行でも「送り手: Web 課（課）」とフィルタと同じ語（CoS/部/課）。 | bb8ce583 |
| W-33 | 中 | 直した。各行に 送り手・「元のタスクを開く」・「案件を開く」、質問と提案には「受信箱で答える」。 | bb8ce583 |
| W-34 | 中 | 直した（主旨）。FetchFrame に `subject`（未決の認可の件数・決めた認可・常設ルール）を渡し、失敗箱の文が区別できる。両方失敗時に上部へ 1 つにまとめる案は残課題。 | bb8ce583 |
| W-35 | 中 | 直した。対象は組織（`/api/org`）から選ぶ Select（空は全員）。組織を読めないときだけ ID の Input に落とす。規則文に例と「広すぎる書き方を避ける」説明、登録済み行は課名と追加日時。 | bb8ce583 |
| W-36 | 中 | 直した。開閉 button の accessible name を「「{見出し}」を展開／閉じる」にし、`aria-controls` で本文領域を指す（見える文字「展開」「閉じる」は name に含まれる）。 | bb8ce583 |
| W-48 | 低 | 直した。段の select を共通 `Select`（--color-input の枠・focus-visible・44px）に、通知へのリンクに focus-visible を付けた。 | bb8ce583 |
| W-49 | 低 | 直した。「通知で報告の知らせを見る」を ScreenFrame の actions から絞り込み行の右へ移し、独立行を占めない。label を「報告元の段」に。 | bb8ce583 |
| W-50 | 低 | 直した。説明文を利用者の行動の文に（「下から上がった報告の本文を開いて読みます。…」「判断待ちの認可は受信箱で答えます。ここでは…」）。 | bb8ce583 |

ほか: 報告本文と元の報告を `max-w-prose-ja`（--container-prose-ja、40em）で行長を抑えた。元の報告に種類・送り手・日時の補足行を足した。生の色・任意値 class は足していない（`sm:w-32`・`max-w-prose-ja` は token/標準 class）。入力欄は共通 `fieldClassName`（--color-input）。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0、`lint` → exit 0（既存 warning 5 件のみ）、`test` → exit 0、`check:boundaries`・`check:parity` → exit 0
- `corepack pnpm@12.6.0 -C web build` 後 `corepack pnpm@12.6.0 -C web e2e e2e/work/reports-approvals.spec.ts e2e/parity/reports.spec.ts e2e/parity/inbox.spec.ts` → 11 passed / 3 skipped（screenshot 用）/ 0 failed、exit 0
- `corepack pnpm@12.6.0 -C web mobile-audit` → exit 1。違反 21 件は全て `/tasks/T1`（9）・`/projects/P1`（8）・`/tasks/T1/changes`（4）の textarea「unnamed control」（mobile-audit.mjs が HTMLInputElement の labels しか見ない既知の誤判定）。**`/reports`・`/approvals` の違反は 0 件**。mobile-audit.mjs はこの leaf の範囲外なので直していない（verify 葉の範囲）。
- post screenshot（fixture 入り、360/390/412/1440）: WU artifacts `qa-fix-reports-post/`（`reports-fixture-*`・`approvals-fixture-*`）。`WEB_SHOTS_OUT=<dir> pnpm -C web e2e e2e/parity/reports.spec.ts e2e/parity/inbox.spec.ts -g "fixture screenshots"` で撮影。pre は critique WU の `qa-qa-work-pre/_reports-*`・`_approvals-*`（偽 daemon に fixture が無く取得失敗の姿のみ）。

## 残課題

- W-10 の確認ダイアログ: parity e2e（`web/e2e/parity/inbox.spec.ts` の /approvals 試験）が「追加」1 クリックで POST 1 件を期待し、parity の期待は書き換え禁止。確認を挟むと parity が落ちるので、送信前の影響表示と直後の取り消しで代えた。確認ダイアログにするには parity 期待の更新を人が決める必要がある。
- W-34 の「両方失敗なら上部に 1 つにまとめる」: subject で区別できるようにしたが、まとめ表示は FetchFrame（`web/components/`、範囲外）側の機能が要る。
- 報告の並び（新しい順か）は API の順のまま。`/reports` は既読を扱わないので未処理/処理済みの区別は通知側。
- 偽 daemon の既定 fixture（`web/e2e/support/fake-daemon.mjs`）に reports・approvals・standing-rules が無く、`pnpm screenshots` の全画面撮影では取得失敗の姿になる。データ入りの姿は parity の fixture screenshot と本 spec で確かめた。

## 提案

- parity の /approvals 試験を「追加 → 確認ダイアログ → 確定」に更新する決定を人に求め、常設ルール追加を ConfirmDialog にする（W-10 の完全な解消）。
