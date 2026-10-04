---
title: task 詳細（/tasks/$id）の観測性 header・木・token 化
tasks: [01M437RJFEAEF8JET7NHF01HBM]
status: running
updated: 2026-10-04
---
# task 詳細（/tasks/$id）の観測性 header・木・token 化

WorkUnit header-tree（第 1 歩）と mobile（第 2 歩、末尾の節）の記録。差分基点は 171c8e02。

## 変更点

- **header**（`overview-view.tsx` の `TaskDetailHeader`、`task-detail-view.tsx` で h1 の直下に置く）:
  題（3 行で省略、全文は title 属性）、状態（StatusBadge）、現在の run（走っている run を優先、無ければ
  最後に始まった run。ID は等幅で省略し run 画面へ link、run の状態も StatusBadge）、次の操作
  （approve / answer / phase_gate / retry のうち今できるものを「承認を待っています」等の link で出し、
  判断 panel・実行 panel へ移る）。同名の button を増やさないため header は link だけにした。
  tab に関係なく出し、detail の query key は既存と共有する（取得を増やさない）。
- **木**（`TaskTree`）: 親 task → この task → 段（計画の stages）→ WU（題・StatusBadge・key・子 task・
  run）、WU に紐づく「main から取り込み integration repair」行（work_unit_id が段の WU に無ければ task 直下）、
  子 task（StatusBadge・ID）。ID は `ShortId`（等幅・1 行省略・title 属性で全文）。execution は
  ExecutionPanel と同じ query key（`taskKeys.execution`）。
- **token 化**: overview-view・decision-panel・integration-repair-panel・execution-panel・task-detail-view・
  timeline-view の neutral/sky/red/amber/indigo と `grid-cols-[...]` を DataList・Badge・semantic token に
  置き換えた。入力欄は `border-input bg-surface`。integration repair は info 枠＋Badge（実装失敗の danger と
  区別）。`IntegrationRepairPanel` に任意の `anchorId` を足した（inbox では付けない＝id 重複なし）。
- **保ったもの**: h1『タスクの詳細 <id>』、判断 panel の button 名・label、`decision-status`（生の状態語）、
  `execution-view`・`routing-panel`・`task-overview`・`integration-repair` の testid、data-tab、URL。
  判断・実行 panel の section に `id`（`decision-panel`・`execution-panel`）を足しただけ。

## before / after の要点

screenshot（run の中間物）: `artifacts/shots/task-detail-{before,after}-{360,390,412,1440}.png`
（spec 内の長い ID・題の fixture。`TASK_DETAIL_SHOT_DIR` を渡すと header の試験が撮る）。

- before: h1 の下はすぐ tab。状態は判断 panel の「状態 running」（生の語）と概要の dl だけ。現在の run は
  概要の末尾の一覧から探す必要があった。WU・段・integration repair は木として出ず、子 task は題の羅列。
- after: h1 の直下に状態・現在の run・次の操作がまとまり、木で親子・段・WU・repair の位置関係が一目で分かる。
  長い ID は省略、長い題は折り返し（header は 3 行・木の自 task 行は 2 行で省略）。360px で横 scroll なし。

## 検査結果（2026-10-04）

- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の warning 4・info 1 は今回の差分外）
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 42 files / 269 tests、node --test 42 pass）
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/task-detail.spec.ts` → 6 passed（新規: header と木の試験 1 件）
- `corepack pnpm@12.6.0 -C web mobile-audit` → `30 path(s) x 4 widths ok`
- FRONTEND_CONTRACT の生の色・任意値 grep を所有 file に当てて 0 件

## 未解決事項

- 360px で tab のラベルが 2 行に折れる件は mobile WU で解消（下の節）。
- 判断 panel の「状態 running」と概要の「状態」は parity 試験（`decision-status` の生の語）のため生の語のまま。

## 要望

- `web/components/ui` に **ShortId / CopyableId**（等幅・省略・title・copy 操作）の primitive が欲しい。今は
  overview-view.tsx の中に置いた。他画面（run・changes・inbox）でも同じものが要る。
- **TreeList**（入れ子の枝線・展開/折りたたみ・キーボード操作つきの木）の primitive。今は ul と border-l で組んだ
  静的な木で、WU が多い計画では折りたたみが欲しい。
- `text-code` は `--color-code`（色）と `--text-code`（寸法）の両方に当たり、色として解釈されて文字が薄くなる。
  styles.css 側で名前を分けるか、DESIGN.md に注意を書いてほしい（今回は `text-label` で回避）。
- `decision-status` を StatusBadge にできるよう parity 試験側の期待（生の語）を見直す余地がある。

## mobile（360〜412）の tab と Drawer（WorkUnit mobile、2026-10-04）

### 変更点

- **区画の切り替え**（`task-detail-view.tsx`）: md 未満で概要 tab の上に「概要・判断・実行・木」の切り替え
  （`fieldset` + `aria-pressed` の button、`data-testid="mobile-sections"`・`data-section`、各 44px 以上）を出す。
  閉じた区画は md 未満でだけ隠し、md 以上は包みを `display: contents` にして header-tree の配置をそのまま保つ
  （`task-detail-tabs.ts` の `mobileSectionClass`）。開く区画は画面の状態で URL は変えない。
- **hash で区画を開く**: header の「次の操作」（`#decision-panel`・`#execution-panel`）と木の link は、
  スマホではその移動先を含む区画を開いて scroll する（`sectionForHash`。同じ hash への再移動も history の key で拾う）。
- **Drawer**（`overview-view.tsx`）: 木の WU 行はスマホで題と状態と「詳細」だけにし、key・段・ID・子 task・run は
  Drawer（木の詳細）で開く。integration repair はスマホでは inline の panel を隠し、木の行の button から Drawer で開く
  （閉じると focus は button に戻る）。desktop は従来どおり行の中・inline。
- **上の 5 tab**: ラベルを 1 行（`whitespace-nowrap`）にし、溢れは nav の中だけで横 scroll（360px で「作業ツリー」が 2 行に折れていた）。
- **保ったもの**: h1『タスクの詳細 <id>』、data-tab・aria-current、判断・実行 panel の button 名・label、
  `decision-panel`・`execution-panel`・`task-overview`・`task-tree`・`integration-repair` の testid、URL。

### before / after の要点

screenshot（run の中間物）: before は `artifacts/shots/task-detail-mobile-before-{360,390,412,1440}.png`、
after は `artifacts/shots/task-detail-mobile-{summary,decision,tree}-{360,390,412}.png`・`task-detail-mobile-drawer-412.png`・
`task-detail-mobile-after-1440.png`（`TASK_DETAIL_SHOT_DIR` を渡すと新しい mobile 試験が撮る）。

- before: 360px で判断・実行・概要・木・repair・run が 1 列に約 4000px 続き、木や判断へ行くには長く scroll した。tab ラベルが 2 行に折れた。
- after: header の下に区画の切り替えがあり、1 区画ずつ（木の区画は約 1500px）。WU の細部と repair は Drawer。desktop の 1440 は before と同じ配置。

### 試験（`web/e2e/parity/task-detail.spec.ts`）

- 新規「スマホ幅の区画 tab と Drawer、長い ID・題、desktop の配置」: `width: 360` から 390・412 で各区画を切り替え、
  `scrollWidth - clientWidth = 0`、区画 button と Drawer の trigger が 44×44 以上、header の長い題は省略され
  title 属性が全文、run の長い ID は省略され title 属性が全文、Drawer の中でも溢れない、header の link が区画を開く、
  1440 では切り替えが隠れ全区画と WU の run link が並ぶことを見る。
- 既存の header・木の試験（360px）は、判断 button と木を見る前に区画を開くよう直した（見る中身は同じ）。

### 検査結果（mobile、2026-10-04）

- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の warning 4・info 1 は差分外）
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 42 files / 270 tests。区画の単体試験 1 件を追加）
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/task-detail.spec.ts` → 7 passed
- `corepack pnpm@12.6.0 -C web mobile-audit` → `30 path(s) x 4 widths ok`
- 生の色・任意値の grep（所有 file）→ 0 件、`git diff --name-only 171c8e02` は所有範囲だけ

### 要望（mobile）

- `web/components/ui` に **SegmentedControl / Tabs**（md 未満だけの区画切り替え、`aria-pressed` か tablist、44px）の
  primitive が欲しい。今は task-detail-view.tsx の中に置いた。run 詳細・案件詳細でも同じ形が要る。
- Drawer に **「区画として常に出す（desktop は inline・mobile は Drawer）」** の variant があると、今の
  「inline は md:contents、mobile は trigger」という二重の置き方を画面ごとに書かずに済む。
- 判断を待つ task ではスマホの既定区画を「判断」にする案（今は「概要」で、header の link から移る）。人の判断を仰ぎたい。
