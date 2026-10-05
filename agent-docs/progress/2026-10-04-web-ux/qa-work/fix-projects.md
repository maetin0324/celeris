---
title: qa-work fix-projects — 案件・案件詳細・文書・board の critique 指摘の修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# qa-work fix-projects — 案件・案件詳細・文書・board の critique 指摘の修正

[qa-work.md](../qa-work.md) の「## critique」で担当が fix-projects の指摘（W-05〜09・W-21〜30・W-37〜43）を重い順に処理した。
commit: `9a8527d3`（画面の修正）・`87b5e3e3`（非 parity e2e・mobile-audit の label 判定）・その次の commit（状態 badge の折り返し）。
post screenshot: WU 成果物 `qa-fix-projects-post/`（`_projects*`・`_board*` の 360/390/412/1440）。pre は critique 葉の `qa-qa-work-pre/`。

## 指摘ごとの結果

### 高

- **W-05 修正**: 仕事の木の題名セルの `whitespace-nowrap` を外し `min-w-48 break-words` に。長い題名が折り返り、1440 で状態・判断待ちの列が見える。状態 badge は折り返さない。狭い幅で状態を題名の下へ移す案は、parity（`[data-tree-task='T2'] [data-status='ready']` が 1 要素であること・木の枠が 390 幅で横 scroll すること）と両立しないため列のまま。
- **W-06 修正**: 文書の保守を、監査＝指摘のある文書の一覧（題名・path・指摘）、整理案＝操作（日本語）と対象の表＋理由、ポリシー＝方針の日本語表示に。JSON は details（既定で閉）の中。
- **W-07 修正**: 「整理案を承認」「承認済み案を適用」「ポリシーを採用」を ConfirmDialog に。対象（操作数・file 数）・影響・戻し方・結果 task の確認先を示し、確定ラベルは「整理案を承認する／整理案を適用する／ポリシーを採用する」。初期 focus は「戻る」（共通部品の既定）。
- **W-08 一部修正・残課題**: 削除ボタンを destructive variant にした。`window.confirm` → ConfirmDialog は、parity `/projects/:id/docs 初期化・保存・削除` が `page.once("dialog")` で受けて「削除」click 直後に DELETE を期待するため、parity の期待を書き換えずには替えられない。parity を直す task で替える。
- **W-09 修正**: 文書一覧・文書・保守の FetchFrame に `subject`（「案件の文書の一覧」「文書 <path>」「文書の保守情報」）。保守画面は 409 を「文書リポジトリがありません → 文書を用意する（文書画面へ）」の Notice に。

### 中

- **W-21 残課題**: 操作節（`project-ops.tsx`）の畳み込み。parity `/projects/:id 案件の操作`・`全 intent` が各 form を開いたまま `getByLabel` で直接埋めるため、details/Drawer へ畳むと parity の期待が変わる。parity と一緒に直す。
- **W-22 残課題**: 根の仕事ごとの操作の出し分け。parity `全 intent` が全操作のボタンを押すため同上。
- **W-23 修正**: board の tier・category・priority を日本語（軽量/標準/最上位、機能/不具合/調査/運用/文書/その他、至急/高/通常/低）。未知の値は「未確認」。編集の select も表示は日本語・値は英語のまま。対応表は `board-model.ts` に置いた（他画面との共通化は web/components 変更になるので範囲外）。
- **W-24 一部修正**: 根の task の集計を `statusView` の日本語ラベルに、DAG の「依存:」を相手の題名に。リポジトリ欄の repo.id（`project-ops.tsx`）は parity が label で引くため残課題。
- **W-25 修正**: 「すべての案件」のとき board の行の題名の下に「案件: <名前>」。案件で絞ると出さない。
- **W-26 修正**: /projects は sm 未満で途中目標・最終更新の列を畳み、題名の下に 1 行で出す。360 で表が枠に収まる（e2e で確認）。
- **W-27 残課題**: 作成フォームの開閉。parity `/projects 一覧・作成・422` が開いたフォームを直接埋めるため。
- **W-28 一部修正**: 一覧の「判断待ち N 件」は `/inbox?project=<id>` へ。詳細の仕事の木の判断待ちは parity が `href="/inbox"` を期待するので残課題。詳細の「ボード」リンクは `?project=<id>` を渡すようにした。
- **W-29 残課題**: 「作業場所を消す」「状態を変える」の確認。parity `案件の操作` が click 直後の送信を期待するため。
- **W-30 修正**: JSON.parse の失敗は送らず、欄に `aria-describedby` で結び付けた `role="alert"` を出す（確認ダイアログにも理由を出す）。

### 低

- **W-37 修正**: 文書・保守の nav リンクを `inline-flex min-h-11 min-w-11 items-center`。
- **W-38 修正**: 保守の 3 節を ProjectSection（見出し・枠）に、「適用」を primary に。
- **W-39 残課題**: checkbox を 20px にすると `mobile-audit` が input 自体の 44px 未満を違反にする（label の当たり判定を見ない）。いったん直して戻した。mobile-audit の判定を label 込みにする共通側の変更と一緒に。
- **W-40 修正**: 一覧の途中目標を「なし」（0 件）・「取得中…」・「取得不可」に分けた。
- **W-41 一部修正**: board の説明文「行の『編集』から…」を削った。狭い幅で案件 select を details に入れる件は残課題（parity の board 試験が案件 select を使う）。
- **W-42 修正**: DAG 枠の inline style `maxHeight` → `max-h-screen`、文書の任意値 grid → `lg:grid-cols-3`＋`lg:col-span-2`、`text-lg` → `text-section`、裸の `rounded border` → `rounded-lg border border-border`。
- **W-43 一部修正**: h1 直下の案件名を `text-title` に（h1 文字列は parity のため不変）。

## mobile-audit の label 判定

`web/scripts/mobile-audit.mjs:65` が `HTMLInputElement` の labels しか見ず、label 付き textarea（/projects/P1 の `project-ops`）を unnamed と誤判定していた。`"labels" in el` で labels を持つ要素一般を見る最小修正をした（兄弟 task qa-ops も同じ修正をしている可能性あり。merge で同一行なら同じ内容）。

## 検証（2026-10-05、HEAD は本記録の commit）

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` exit 0
- `pnpm -C web typecheck` exit 0、`lint` exit 0（既存の warning のみ）、`test` exit 0（57 files / 347 tests）、`check:boundaries` exit 0
- `pnpm -C web build` 後 `pnpm -C web e2e` exit 0（168 passed / 8 skipped）。`e2e e2e/work/projects.spec.ts e2e/parity/projects.spec.ts` 13 passed
- `pnpm -C web mobile-audit` exit 1。違反は `/tasks/T1`（`a#- 17x44`、web/features/tasks の範囲外）だけで、/projects・/projects/P1・/docs・/maintenance・/board は 0 件。
- 生の色・任意値 class: `git diff c439eb86 -- web/features/projects | grep '^+' | grep -E '#[0-9a-fA-F]{3,6}|-\[' ` は 0 件。

## 残課題のまとめ

W-08（ConfirmDialog 化）・W-21・W-22・W-27・W-28（詳細側）・W-29 は parity e2e の期待（web/e2e/parity/projects.spec.ts）を変えないと直せない。parity を書き換えてよい task で一括して直す。W-39 は mobile-audit の 44px 判定（label 込み）の共通側の変更待ち。/tasks/T1 の tap target は担当外。
