# web/ SPA: 画面下の固定タブバーと「目的の中身が最初の 1 画面に入る」方針

- 日付: 2026-10-06
- 状態: 実装済み（task 01M46VAZ0G1A6DN7BBK2DQ7X36）。D2 は付記 2026-10-08 で改訂（task 01M4CRGVA0Z9T2SQ2QR4XFR6H5）
- 関連: ADR-0055 D2（旧 GUI の下部タブ）、ADR-0081（web SPA）、web ADR 2026-10-04（受信箱・通知）、docs/frontend/DESIGN.md「幅ごとの規則」

## 背景

人の要望（2026-10-06）。(1) web 画面を旧 GUI（gui/）と同様に画面下のメニューバーで操作したい。(2) 知識ベースの閲覧などが縦に長すぎ、目的の中身まで長く scroll する。

## 決定

### D1. 下部タブは md 未満に出し、md 以上の側面 nav は変えない

旧 GUI は lg 未満で下部タブを出していたが、web の shell は狭い/広いの境を md（48rem）に置いている（DESIGN.md「幅ごとの規則」、側面 nav は md から）。境を 2 つ持つと 48〜64rem で側面 nav と下部タブが同時に出るので、下部タブも md 未満とする。md 以上の側面 nav・header・接続状態は一切変えない。

狭い幅の上の帯にある「メニュー」ボタン（主要 nav の全一覧）は残す。下部タブは片手で届く主要 4 項目の近道、メニューは全項目の一覧という役割分担にする（既存の e2e と操作の覚えも保つ）。

### D2. 主要 4 項目と「その他」

主要 4 項目は **ホーム `/`・受信箱 `/inbox`・タスク `/tasks`・案件 `/projects`**（`nav-items.ts` の `mobileTabs`）。選び方は nav-items.ts の優先順（ホーム・受信箱・通知・タスク・案件…）と旧 GUI の並び（Console・ボード・案件・認可）の折衷:

- 旧 GUI の Console はホーム、認可は受信箱に当たる（未決の認可は受信箱で答える。web ADR 2026-10-04 D4）。
- 通知は旧 GUI と同じく「その他」のシートに置き、未読は受信箱と同じ badge の仕組みで「その他」の点（未読があり、現在地がシート内でないとき）と通知の行に出す。
- 旧 GUI のボードは、web ではタスクの別の見方なのでタスクを主要に置き、ボードはシートに置く。（この項は付記 2026-10-08 で改めた。主要はボード、タスクはシート）

5 つ目は「その他」ボタン（aria-expanded）。残りの項目を navItems の順・group の見出し付きで、Radix Dialog の下からのシート（role=dialog・aria-modal・Escape で閉じ・閉じたら focus を戻す・遷移で閉じる）に出す。シート内の項目が現在地なら「その他」を現在地の色にする（`data-active`）。

### D3. 本文が隠れない仕組み（token）

タブの高さは `--spacing-tabbar`（4rem）。覆う量 `--shell-bottom-inset` = タブ + `env(safe-area-inset-bottom)`（md 以上は 0）を styles.css に置き、本文と Console の置き場を囲む列の下余白（`pb-shell-inset`）と、Console の送信欄の `bottom`、「最新へ」ボタンの位置に足す。safe-area はタブ側が受け持ち、送信欄で二重に空けない。現在地は `aria-current="page"` と上の線（border-primary）・太字・text-primary で示し、色だけに頼らない。タップ領域 44×44 以上、focus-visible の outline、dark mode は token に任せる（web に dark mode の切替はまだ無い）。

### D4. 縦の長さ: 「目的の中身が最初の 1 画面に入る」を gate にする

各画面の「目的の中身」（選んだ知識の本文、一覧の先頭行、タスクの目的、ログ本文）が、scroll 前の最初の 1 画面（360×800 と 1440×800。狭い幅では下部タブが覆う帯を除く）に入ることを Playwright で決定的に確かめる（`web/e2e/support/first-screen.ts` の `expectInFirstScreen`、`web/e2e/work/first-screen-*.spec.ts`）。詰め方の規則:

- 広い幅（lg 以上）で一覧と中身を併置する画面は 2 ペイン。一覧は独立 scroll（sticky）、選択行は `aria-current` と背景、中身は右ペイン。
- 狭い幅で選択中は中身を h1 の直下に出し、一覧は隠して「一覧に戻る」で戻る（DOM 順と見た目の順を揃える）。一覧から選び直した時だけ中身の見出しへ focus を移す（直接開いた初回は shell の h1 focus に任せる）。
- 上部の説明・統計は削るか 1 行要約に、リンク列は ScreenFrame の `actions` に、フィルタは 1 行の toolbar（横 scroll の chip・折りたたみ）に。メタ（出典・scope・更新日など）は 1 行（`MetaLine`）にし、一覧と中身で縦に重複させない。
- 選択・フィルタの状態は URL（`?path=`・`?name=`・`?tab=`・`?status=` 等）で復元できる。

### D5. 旧 GUI は変えない

gui/ は変更しない。旧 GUI の `MobileTabBar` は参照のみ。

## 結果と残課題

- 直した画面と before/after の棚卸しは `agent-docs/progress/2026-10-06-web-tabbar-first-screen.md`。
- 残課題: /tasks/T1 の 360 は目的の先頭 3 行までが入る（題の全文・状態の縦積みが上に残る）。/org/cos は人の詳細を出さず会話だけ（別の判断が要る）。web の dark mode は未導入のため dark の試験は無い。

## 付記 2026-10-08: 下部タブの 3 列目をタスクからボードに替える（D2 の改訂）

人の決定（2026-10-08、CoS チャットの依頼「web 画面のスマホ版の下のメニューバーに関して、真ん中のボタンはタスク一覧ではなくボードが見れるようにしてください」）により、D2 の「タスクを主要に置き、ボードはシートに置く」を次のとおり改める。上の D2 の本文は経緯として残す。

- 主要 4 項目は **ホーム `/`・受信箱 `/inbox`・ボード `/board`・案件 `/projects`**、5 つ目は「その他」。下部タブの並びは ホーム・受信箱・ボード・案件・その他（`nav-items.ts` の `mobileTabs`）。ボードの label は「ボード」。
- タスク `/tasks` は「その他」のシートに移る。シートの項目は従来どおり navItems から主要 4 項目を除いたものを navItems の順・group の見出し付きで出す（タスクのために順を足さない）。`/tasks` を開いているときは「その他」が現在地の色（`data-active`）になる。タスクに未読 badge は無い。
- 理由: スマホでは状態ごとの列でタスクを見渡すボードが人の主な使い方で、旧 GUI の下部タブ（Console・ボード・案件・認可）とも揃う。タスク一覧はシートとメニューから届く。
- 変えないこと: 列数 5、タップ領域 44×44 以上、ラベルは 1 行、`env(safe-area-inset-bottom)` の扱いと `--shell-bottom-inset`（D3）、md 以上の側面 nav（項目・順・見た目）、navItems の順、上の帯の「メニュー」ボタン（D1）。
- 試験: `web/e2e/shell/tabbar.spec.ts` の並び・現在地（/board で「ボード」が `aria-current="page"`、/tasks で「その他」が `data-active`）と、/board が 360px で最初の 1 画面に目的の中身が入ること（D4 の `expectInFirstScreen`）で確かめる。実装は同じ task の tabbar 葉。
