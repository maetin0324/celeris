---
title: fix-r6 narrow — 狭い幅の発見性と走査性（providers・T1 tab・graph・changes・会話枠）
tasks: [01M45WS2HYW31J33MEY59DY3HE]
status: done
updated: 2026-10-05
---

# fix-r6 narrow

基点は 297ada61。after-r4（`/local/celeris/data/workspaces/01M45PRPACBEP4X1FWPNP4FRMG/wu/fix-r5/artifacts/after-r4`）の目視記録（record-r5/look-a・look-b・look-c）で要修正・保留になった 7 点を直した。変更は web/features（ops/providers・tasks・changes・home・console）、web/components/shell/shell.tsx、web/routes/index.tsx（ホームの未確認の判定だけ）、web/e2e。fixture・web/e2e/support/・web/scripts/screenshots.mjs・crates・gui・web/server・API 型は変えていない。

撮った画像は `/local/celeris/data/workspaces/01M45WS2HYW31J33MEY59DY3HE/artifacts/after-r6/`（この run の artifacts。基本画面は `screenshots.mjs --only`、状態変種は `--states` で `states/` に 116 枚、ホームの viewport 撮影は `viewport/`）。下の表の画像は 1 枚ずつ開いて見た。

## 7 点の原因と修正

| # | 画面 | 原因 | 修正 | 確かめた画像 | 固定した e2e |
| --- | --- | --- | --- | --- | --- |
| 1 | /providers 360・390・412 | `ProviderStatusTable` が 6 列の表のままで、枠は `overflow-x-auto` だが手がかりが無く、同時実行・前回の確認が右の外にあった | 640px（sm）未満は行を積む。見出し行は `max-sm:sr-only`、各 `tr` を flex-wrap、1 行目に「実行枠 id＋状態 badge」、下に「道具: / 受ける段: / 同時実行: / 前回の確認:」の label: value（label は `aria-hidden`、sm 以上は隠す） | `_providers-360.png`・`_providers-390.png`・`_providers-412.png`（4 項目が見出し付きで枠内）、`_providers-1440.png`（6 列のまま） | `narrow-r6.spec.ts` (1) 360/390/412: 枠の scrollWidth ≤ clientWidth+1、4 項目が viewport 内で右端 ≤ 幅・見出し付き。1280: 列見出しが見え積みの見出しは隠れる |
| 2 | /tasks/T1 360 | tab の `ul` が `w-max`、各 tab が `px-3` で合計約 342px > 本文幅。末尾の「成果物」が切れた | sm 未満は tab の左右余白を `px-2` に詰め、`ul` を `flex-wrap`（さらに狭い幅では折り返す）。nav の `overflow-x-auto` をやめた | `_tasks_T1-360.png`（5 tab が 1 行に全文） | (2) 360: 5 tab が ratio 1 で viewport 内、nav が横に溢れない |
| 3 | /graph 360〜412 | graph の枠は `overflow-auto` で横に続くが、注記・影・scroll bar のどれも見えなかった | 枠が横に溢れている間だけ「横に続きます。枠を左右に動かすと残りの task が見えます。」を出し（枠の `aria-describedby`）、まだ見えていない側の端に影（`graph-edge-end` / `graph-edge-start`）を置く。溢れの判定は scroll・ResizeObserver | `_graph-360.png`・`_graph-390.png`・`_graph-412.png`（注記と右端の影）、`_graph-1440.png`（収まるので注記なし） | (3) 360/412: 注記・右の影あり、右端まで scroll すると右の影が消え左の影が出る。1440: 注記・影なし |
| 4 | /tasks/T1/changes 狭い幅 | 各行に全文 path を `break-all` で出しており、共通 prefix（約 70 字）が 15 行とも 4〜6 行に折れ、ファイル名が埋もれた | `splitChangedPaths`（diff-lines.ts、unit test あり）で 2 件以上に共通の dir を `/` 単位で取り出し、表の上に「共通の場所: …」を 1 度だけ出す。各行の button はファイル名を先に、残りの dir を薄く下に。全文 path は `title` と button の名前（`aria-label`）に残す | `_tasks_T1_changes-360.png`・`-390.png`・`-412.png`（共通の場所 3 行＋各行 1 行のファイル名）、`-1440.png` | (4) 360/1440: 共通の場所が 1 つ、各行のファイル名＝basename で 1 行、名前と title が全文 path。`parity/task-detail.spec.ts` の「src/long.ts を含む」は button の名前で見る形に追従 |
| 5 | / 1440・電話幅 | 会話枠は末尾へ送るので、先頭に見える block の見出し行が sticky の宛先行の下端で半分切れて見えた | retry で帯を撤去し、初回・追記・「最新へ」の末尾追従後に block 境界へスクロール位置を揃えた。宛先行に隠れる block は次の block まで送る。手で過去を読んでいる間は位置を変えない | 変更前 `viewport/rich-_-1440-top.png`・`rich-_-360-top.png`、変更後 `viewport/rich-_-1440-top-r2.png`・`rich-_-360-top-r2.png`・`rich-_-412-top-r2.png`（先頭に見える見出しと時刻が全文表示） | (5) 360/1440: sticky の宛先行の下端以上に、先頭に見える block の上端があることを座標で検査 |
| 6 | stale の / 360 | 最低の高さ 160px が枠全体に掛かっており、宛先行（約 52px）と fixed の送信欄（約 83px）に食われて会話は約 40px。さらに fixture の再試行ごとに接続状態が「確認中」を挟み、黄帯が消えて出るたびにページの高さが跳ね scroll が先頭へ戻っていた | 最低の高さを「会話が見える 160px＋宛先行＋送信欄」にし（足りない viewport はページの scroll に任せる）、末尾送りの余白（slack）は枠の今の位置でなく「ページ末尾での枠の下端」から測る。ホームの未確認は一度「再接続中」「未接続」になったら open / unauthorized まで保つ（`features/home/use-unconfirmed.ts`、unit test あり） | `states/stale-_-360.png`、`viewport/stale-_-360-end.png`・`stale-_-390-end.png`（ページ末尾で会話が約 160px 見え、最後の block が送信欄のすぐ上） | (6) stale 360: ページ末尾で宛先行の下端〜送信欄の上端が 150px 以上、最後の block が送信欄より上、「確認中」→「再接続中」の 1 巡（出来事待ち）の間も黄帯と scroll 位置が保たれる |
| 7 | loading の /providers 360 | 接続状態の語「接続を確認中」（6 字）で badge が他より約 30px 長く、header が 2 段に折れた | 語を「確認中」にし、他の状態（接続済み・再接続中・未接続）と同じ 3〜4 字に揃えた | `states/loading-_providers-360.png`（Celeris・接続状態: 確認中・メニューが 1 段） | (7) loading 360: 「接続状態: 確認中」、メニューと接続状態の縦範囲が重なり header は button 1 つ分の高さ |

## 証拠

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile && … typecheck && … lint && … test && … build && … mobile-audit` | exit 0（lint は既存の warning 5、vitest 356 passed、node test 42 pass、mobile-audit 31 path × 4 幅 ok） |
| `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /' --retries=0` | exit 0（200 passed / 8 skipped。skip は既存のもの、新規 14 件を含む） |
| `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | exit 0（95 passed、axe・mobile-gate 含む） |
| `test -z "$(git log --format= --name-only 7eab1be6a4e3..HEAD --not main \| grep -E '^(crates\|gui\|web/server\|docs/api)/')" && git diff --quiet HEAD -- crates/ web/server/` | exit 0 |

## 未解決事項

- 撮影は `--only`・`--states` と、ホームのページ末尾を見るための一時 spec（commit していない）で行った。after-r6 一式の撮り直しと目視判定は後段（記録）の担当。
- 接続状態の badge 自体は再試行のたびに「確認中」と「再接続中」を行き来する（transport が初回接続前の再試行を connecting とするため）。ホームの黄帯は保つようにしたが、header の語の行き来は web/api/realtime の範囲なので触っていない。

## 提案

- `transport.ts` で一度失敗した後の再試行は `reconnecting` のままにすると、header の語の行き来と他画面の stale 表示の点滅もなくなる（ホームの `useUnconfirmedConnection` はその後で不要になる）。
- 狭い幅で行を積む表が /artifacts・/projects/P1・/providers の 3 画面になった。`components/ui/table` に「sm 未満で積む」variant と、横に溢れる枠の注記・端の影（graph の `useHorizontalScroll`）を共通部品にすると、各画面の `max-sm:` の並びを減らせる。
