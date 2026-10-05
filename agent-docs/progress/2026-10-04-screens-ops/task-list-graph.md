# task 一覧・作成・依存グラフの画面確認
---
tasks: [01M437TBQX3NHZ269B8ETY77XM]
completed: 2026-10-04
---

## 変更点

- `/tasks` は状態・担当・現在の run 情報の有無・更新時刻を密に並べ、360px では行内を折り返す。長い名前と ID は省略表示し、`title` 属性で全文を保持する。
- `/tasks/new` は内容と受け入れ条件を分け、入力欄の境界に `--color-input` を使う。条件の種類に応じた説明と送信エラーを入力の近くに置く。
- `/graph` は状態の文字付き badge と意味色の帯で節点を示し、広いグラフのスクロールを canvas 区画内に収める。
- `web/e2e/parity/tasks.spec.ts` に `/tasks/new` と `/graph` の 360px 試験を追加した。先行する `/tasks` の長文試験と合わせ、3 画面のページ横溢れを検査する。

## before / after と自己レビュー

基点 `171c8e02` と統合後の画面を 360 / 390 / 412 / 1440px で撮影した。画像は run の `screenshots/before` と `screenshots/after` にある。共通 screenshot fixture では `/tasks` と `/graph` の取得が失敗状態になるため、その状態の見た目を比較した。データ表示時の行・節点は parity 試験の偽 daemon で検証した。
さらに同じ偽 daemon で長い title の一覧と複数節点の graph を 360 / 1440px で撮り、`screenshots/loaded` に保存した。360px の一覧は行内を折り返し、graph は次の節点だけが canvas の端で隠れ、ページ全体は横に動かないことを目視でも確認した。

| 画面 | before | after / 判断 |
|---|---|---|
| 一覧 | 横幅の狭い画面でもフィルタの後に取得エラーを表示 | 状態・担当・run・更新を一覧にまとめ、長文は省略と全文属性を両立。検索・絞り込み・再試行の順序が明瞭。360px で行を card 化せず折り返す |
| 作成 | 名前・目的・条件が平坦に続く | 内容と受け入れ条件を Panel で分け、条件の補足とエラーの場所を明確化。主操作を最後に残し、入力欄の枠と 44px target を確保 |
| graph | root / depth と取得エラーが連続 | 状態文字と意味色で節点を識別し、横に広いグラフは canvas のみがスクロール。ページは 360px で横に動かない |

`ui-ux-quality-gate` の自己レビューでは、運用者の「対象を探す→状態を読む→詳細へ進む」「条件を入力→作成→結果を確認する」「依存を追う」という流れを確認した。3 画面とも見出し、入力の名前、URL は既存の parity 契約を維持。mobile-audit と axe e2e によって操作領域とアクセシビリティを確認した。長い title は見た目を崩さないが、全文は `title` 属性とリンクのアクセシブル名で参照できる。

## 要望

- 共通 screenshot fixture の `/tasks` と `/graph` で取得成功・長文・複数節点を撮れる設定が欲しい。現在の定型撮影はエラー表示だけになり、通常データ状態の視覚差分を確認しづらい。`web/e2e/support/` は本 task の所有範囲外なので変更していない。
- 一覧 API に現在の run が無いため、一覧では「情報なし」と表示する。run を一覧で示すには API 側の追加が必要。

## 検査

| コマンド | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web build` | 成功 |
| `corepack pnpm@12.6.0 -C web typecheck` | 成功 |
| `corepack pnpm@12.6.0 -C web lint` | 成功（既存の warning 4 件・info 1 件） |
| `corepack pnpm@12.6.0 -C web test` | 成功（vitest 270 件、server 42 件） |
| `corepack pnpm@12.6.0 -C web check:parity` | 成功 |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 成功 |
| `corepack pnpm@12.6.0 -C web check:secrets` | 成功（build 後に実行） |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/tasks.spec.ts` | 成功（7 件） |
| `corepack pnpm@12.6.0 -C web mobile-audit` | 成功（30 経路 × 4 幅） |
| `corepack pnpm@12.6.0 -C web screenshots --out <run artifacts>/screenshots/after --only <route>` | 成功（3 画面 × 4 幅。before も基点を別ビルドして撮影） |
| `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'` | 成功（177 件成功、8 件 skip） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 成功 |
