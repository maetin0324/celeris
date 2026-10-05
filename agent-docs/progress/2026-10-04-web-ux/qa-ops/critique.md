---
title: ops 画面群（task 一覧・graph・作成・task 詳細・changes・files・run ログ・Console・成果物）pre-screenshot critique
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: critiqued
updated: 2026-10-04
---

# ops 画面群 visual QA — pre 撮影の critique

screenshot は `artifacts/qa-ops-pre`（build 171c8e02 系の統合済み HEAD、`corepack pnpm@12.6.0 -C web build` 後）。
`corepack pnpm@12.6.0 -C web screenshots --out <dir> --states` で 116 枚（8 状態 × 代表画面 × 4 幅）、
加えて状態の代表画面に含まれない 4 画面（`/tasks/new`・`/tasks/T1/changes`・`/tasks/T1/files`・`/artifacts`）を
`--only` で rich 既定 state のまま 4 幅ずつ（16 枚）追加撮影した。合計 132 枚。コードは変更していない
（`git status` 差分ゼロ）。

ui-ux-quality-gate skill の自己点検（generic AI dashboard 化・Celeris 固有の情報構造・friction・keyboard/focus/
contrast/touch target）を通し、`docs/frontend/UX_AUDIT.md` と `agent-docs/progress/2026-10-04-screens-ops/*.md`・
`agent-docs/progress/2026-10-04-web-ux/qa-fixtures.md` の既存所見を突き合わせて、現在の HEAD でまだ再現するものだけを
指摘として残した（既に直っている旧 friction は除外し、その旨を各画面の節に書く）。

## 画面ごとの所見

### task 一覧 — `/tasks`（`web/features/tasks/task-list-view.tsx`）

状態・担当・run・更新を密な一覧にまとめる構成は Celeris 固有の運用情報（担当なし・run 情報なし等）を一目で示せており、
generic な汎用テーブルにはなっていない。長文・150 件（`many`）・長い ID の 3 状態は折り返し・省略 + `title` 属性で
横溢れが無く良好（`empty`・`many`・`long-text`・`long-id`・`forbidden` の 5 状態を screenshot で確認）。

- **major** — status 絞り込みの 8 チップ（`draft`/`ready`/`running`/`blocked`/`reviewing`/`done`/`failed`/`cancelled`）が
  生の英語コードのまま表示される（`web/features/tasks/task-list-view.tsx:13` `STATUSES` 定義、`:147-160` の
  `{s}` 描画）。同じ status を表示する `StatusBadge`（`web/components/ui/status-badge.tsx:33-40` の `statusLabel`、
  例 `ready` → 「実行待ち」）はすでに日本語訳を持つのに、この絞り込みチップだけ再利用していない。一覧の行は
  日本語バッジで状態を示すのに、絞り込み UI だけ英語という一貫性のずれが、画面全体を「generic な内部ツール」に
  見せている。`docs/frontend/UX_AUDIT.md:42` の「8 個の英語 status 選択」friction がそのまま現在の HEAD でも再現する。
  チップの `value`（parity test が `selectOption`/`check` で使う生の値）は変えずに、可視テキストだけ
  `statusView(s).label` に差し替えれば直る。

### 依存グラフ — `/graph`（`web/features/tasks/graph-view.tsx`）

節点はすでに `<Link>`（keyboard で tab 移動・`focus-visible:outline` あり）で task 詳細へ飛べ、状態は
`StatusBadge` の文字 + 左帯の色の二重表現、辺は矢印付きの `<line>` で向きが分かる。`docs/frontend/UX_AUDIT.md:192`
が指摘した「`<title>` だけでリンクでもボタンでもない」「色分けが無い」「矢印が無い」は現在の HEAD ではすべて解消済み
（旧 friction として記録のみ残し、指摘からは外す）。

- **major** — 絞り込みフォームの `root`/`depth` ラベルが生の API クエリパラメータ名のまま、日本語訳がどこにも無い
  （`web/features/tasks/graph-view.tsx:68-78` の `<label>root<input name="root" /></label>` と同形の `depth`）。
  create 画面の条件種類（下記）のように補足説明すら無く、ops 画面の中で最も内部語がそのまま露出している箇所。
  `empty` 状態の screenshot（360px・1440px）で確認。

### タスクの作成 — `/tasks/new`（`web/features/tasks/create-screen.tsx`）

内容と受け入れ条件を Panel で分け、主操作（タスクを作成）を最後に残す構成は良好。入力欄の枠・送信エラーの位置も
`FRONTEND_CONTRACT.md` に沿う。`docs/frontend/UX_AUDIT.md:48` の「条件 1 の種類が既定で human だが意味の説明が無い」
friction は解消済み（select の下に「人が確かめる内容」等の日本語ヒントが付いている。現在は指摘から外す）。

- **minor** — 条件の種類 select は API の型名そのもの（`command`/`artifact_exists`/`reviewer`/`human`）を可視の
  主ラベルとして使う（`web/features/tasks/create-screen.tsx:15-22`、コードに「option の値と文字は API の型名のまま
  （parity e2e が selectOption で選ぶ）」と明記された意図的な設計）。直下に日本語ヒントがあるため実害は小さいが、
  選択済みの値を select の外（確認画面やエラー表示）で振り返る場面があれば英語のまま出てしまう。select の可視
  ラベルだけでも日本語＋英語併記（例: 「human（人が確かめる）」）にできないか検討の余地がある。

### task 詳細 — `/tasks/$id`（`web/features/tasks/overview-view.tsx`・`task-detail-view.tsx`）

概要カード（状態・現在の run・次の操作）と 5 tab（概要/判断/実行/木/timeline・変更・作業ツリー・成果物）の構成で、
Celeris 固有の「task 木」「次の操作」「run の状態」が一目で読める。tab ラベルは 390px でも折り返さず 1 行に収まり、
`task-detail.md` の mobile 対応がこの HEAD に反映されている（旧 friction「5 tab のラベルが折り返す」は解消済み、
指摘から外す）。長文・長い ID の 2 状態も横溢れなし。

- **major**（touch target） — task 木・概要カードの run へのリンク（`ShortId` 表示の短縮 ID、例 `R1`）が
  `min-h-11` のみで `min-w-11` を持たず、文字幅がそのままタップ領域の幅になる（実測 17×44px 相当）。2 箇所:
  `web/features/tasks/overview-view.tsx:94-99`（概要カードの「現在の run」）と `:390-399`（task 木の WU 行の
  run リンク）。`agent-docs/progress/2026-10-04-web-ux/qa-fixtures.md` の mobile-audit 所見で既に指摘されていたが
  その WU の範囲外のため未修正のまま現在の HEAD に残っている。44×44px の目安を満たすには `min-w-11` を足すか、
  リンクの padding を広げる。

### 変更 — `/tasks/$id/changes`（`web/features/changes/changes-view.tsx`）

repo ごとの区画・差分ファイル一覧・取り込み操作という Celeris 固有の「変更の危険度を判断して取り込む」流れが
画面から読める。長い path は折り返し + `title` 属性で全文参照でき、360px でも横溢れなし
（`changes-files.md` の verify 済み試験のとおり、現 screenshot でも再確認）。`discard`（破棄）選択時は
`destructive` variant の赤系ボタンに変わり、取り返しがつかない操作の確認チェックボックスも必須になっており、
`docs/frontend/UX_AUDIT.md:66` が指摘した「破棄も他の取り込み方法と同じ扱い」friction は解消済み（指摘から外す）。

- **minor** — 取り込み結果の `integration.state`（例 `merged`/`conflict`/`failed`）をバッジに生の英語のまま表示する
  （`web/features/changes/changes-view.tsx:97-98`・`:337`）。コードコメントに「衝突・git の失敗は 200 で返るので
  integration.state をそのまま出す」と意図が明記されており、git 操作に通じた運用者向けの正確性を優先した判断と
  読める。ただし `tone`（色）だけでなく文字も意味が伝わるよう Badge の `label` を保ちつつ、日本語の短い補足
  （例 `conflict（衝突）`）を添える余地はある。

### 作業ツリー・成果物 — `/tasks/$id/files`（`web/features/files/task-files-view.tsx`）

深い階層・長い file 名でも `Table` のレイアウトが崩れず、`changes-files.md` の 360px 長い path 試験で検証済みの
挙動を screenshot でも確認した。`docs/frontend/UX_AUDIT.md:60` の「空なのか取得失敗か判断しにくい」friction は
`FetchFrameView` の `empty`/`error` 分離（本 critique の「状態別 screenshot の欠落」節を参照）により設計上は解消
されているはずだが、今回の撮影では状態別の見た目を直接確認できなかった（下記参照）。この画面固有の新規指摘はなし。

### run ログ — `/tasks/$id/runs/$runId`（`web/features/runs/run-log-view.tsx`）

`stale`（SSE 切断）状態の screenshot で、状態・harness（`codex (standard)`）・開始・所要時間を `DataList` で、
各行を `agent`/`コマンド`/`エラー` 等の文字 Badge で区別する構成を確認した。run の実行中/完了・判断の文脈が
Celeris 固有の言葉で読め、汎用ログビューアーには見えない。接続切断は shell 上部の「接続状態: 再接続中」だけで
伝わり画面本体にバナーは出ないが、これは `states.ts` のコメントどおりの意図した設計（取得済みデータを保持し、
画面ごとの `StaleState` は別の文脈用）。新規指摘はなし。

### Console — `/`（`web/features/console/console-view.tsx`）

発言・返事・run の作業を種類別の文字 Badge + 時刻で並べ、progress の折り畳み（「手順 2 件」）がある。
`run-console.md` の記録どおり生の色（bg-blue-50 等）は見られず、送信欄・宛先表示も Celeris 固有の文脈
（宛先: CoS）が読める。`stale` 状態でも既存の会話は保持され、新規指摘はなし。

### 成果物 — `/artifacts`（`web/features/artifacts/artifacts-view.tsx`）

- **minor** — 初期表示は「案件を選ぶ」の select のみで、案件を選ばない限り成果物が一件も見えない。
  `docs/frontend/UX_AUDIT.md:90` の「最近の成果物へ直接進めない」friction が現在の HEAD でもそのまま再現する
  （baseline screenshot で確認、360px/1440px とも同じ構成）。task 詳細の「成果物」tab から来た場合は対象が
  決まっているため問題ないが、shell の「成果物」から直接入った運用者には一段余分な選択が必須になる。

## keyboard・focus・contrast・touch target の横断確認

- focus リング: `/graph` の節点・task 詳細の run リンクなど `focus-visible:outline-2 outline-offset-2 outline-ring`
  が主要なリンク・ボタンに一貫して付いている（コードで確認）。
- contrast・axe: `docs/frontend/UX_AUDIT.md` 系の verify 記録（`changes-files.md`）で axe critical/serious 0 件が
  直近 HEAD 相当で確認済み。本 WU では axe を再実行していない（screenshot と記録作成のみの範囲のため）。
- touch target: 上記「task 詳細」の run リンク 2 箇所が唯一の具体的な未解消の指摘。他の主要ボタン・チェックボックス
  （status チップの当たり判定、discard の確認 checkbox 等）は `min-h-11 min-w-11` 相当を確保している。

## 残課題（本 WU の範囲外。次段の fix 葉・verify 葉へ引き継ぐ）

- **screenshot tooling のタイミング問題**（範囲外: `web/scripts/screenshots.mjs`、`web/e2e/support/` 配下）:
  `--states` のループは `page.goto()` 直後に screenshot を撮り、ネットワーク往復を要する `loading`（`hold: {}`
  で保留）と `error`（503 fault、fake daemon 経由）の 2 状態は、`/tasks`・`/tasks/T1`・`/graph`・`/providers` の
  いずれでも常に「読み込み中…」文言や赤枠のエラー文言が描画される前の空スケルトン box（`bg-neutral` の矩形のみ）
  で撮影されてしまう。原因は `web/components/fetch-state/delay-tracker.ts` の 1 秒デバウンス（`SHOW_AFTER_MS`）と
  fake daemon 経由のネットワーク往復に screenshot 側の wait が無いこと。`forbidden` 状態は `page.route` による
  in-browser 即時応答のため正しく赤枠＋「一覧へ戻る」ボタンが写る。この問題のため、本 critique では `loading`/
  `error` の実際の見た目を screenshot からは評価できておらず、ソースコード（`web/components/fetch-state/
  fetch-frame.tsx`）の読み取りで代替した。fix は screenshot 側（`--states` ループに `waitForSelector` 等）が必要で
  本 WU の範囲外のため、followup として記録する。
- **状態別 screenshot の欠落**（範囲外: `web/e2e/support/states.ts` の代表画面設計。既存・merge 済みの仕様）:
  `states.ts` の各状態は画面群ごとの代表 1〜2 画面だけを撮る設計のため、`/tasks/$id/changes`・
  `/tasks/$id/files`・`/tasks/new`・`/artifacts` の 4 画面には `empty`/`many`/`loading`/`error`/`stale`/
  `forbidden` の screenshot が一枚も無い（rich 既定の baseline のみ今回追加撮影）。この 4 画面の状態別の見た目は
  ソースコード（`FetchFrameView` の呼び出し）の読み取りでしか確認できていない。`states.ts` を拡張する場合は
  `screens.ts` の台帳行や `check:parity` には影響しない範囲で行う必要がある（過去の知見: 兄弟 WU が触る想定が
  ないファイルのため、拡張するなら専用の WU で）。

## 指摘の一覧（重大度順）

| # | 画面 | 重大度 | 対象ファイル | 要旨 |
|---|---|---|---|---|
| 1 | task 一覧 `/tasks` | major | `web/features/tasks/task-list-view.tsx:13,147-160` | status 絞り込み 8 チップが生の英語コードのまま。`StatusBadge` の日本語訳を再利用していない |
| 2 | 依存グラフ `/graph` | major | `web/features/tasks/graph-view.tsx:68-78` | `root`/`depth` ラベルが生の API パラメータ名のまま、補足説明も無い |
| 3 | task 詳細 `/tasks/$id` | major | `web/features/tasks/overview-view.tsx:94-99,390-399` | run へのリンクが `min-w-11` を持たず、タップ領域が横に狭い（17×44px 相当）。2 箇所 |
| 4 | タスクの作成 `/tasks/new` | minor | `web/features/tasks/create-screen.tsx:15-22` | 条件種類 select の可視ラベルが API 型名のまま（補足ヒントは別途あり） |
| 5 | 変更 `/tasks/$id/changes` | minor | `web/features/changes/changes-view.tsx:97-98,337` | 取り込み結果 `integration.state` が生の英語のまま（意図的だが日本語補足の余地あり） |
| 6 | 成果物 `/artifacts` | minor | `web/features/artifacts/artifacts-view.tsx` | 案件選択必須で直近の成果物に直接進めない（既存 friction、未解消） |

重大度の基準: critical = 主要な操作が成立しない・情報の危険度誤認につながる、major = generic dashboard 化や
touch target 未達など複数状態・複数箇所に影響する一貫性/アクセシビリティ上の不備、minor = 実害は限定的だが
Celeris 固有の言葉遣いの徹底という観点で改善余地がある点。今回は critical 相当の指摘は無かった。
