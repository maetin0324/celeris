# web/ の frontend contract（agent が守る規則と検査コマンド）
---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

`web/` を変える agent と人が守る規則。値と見た目の正本は [DESIGN.md](DESIGN.md)、画面台帳と現状は [UX_AUDIT.md](UX_AUDIT.md)、技術の枠は [ADR-0081](../adr/0081-web-spa-frontend.md) と [web-0004](../web/adr/web-0004-design-system.md)。本書は「何を破ったら差し戻すか」と「何で確かめるか」だけを書く。規則に合わない値が要る場合は、画面で迂回せず DESIGN.md の token へ理由と利用箇所を加える提案をする。

## 規則

1. **色は token だけ**。feature・route・component に生の色を書かない。禁止するもの: `#hex`、`rgb()` / `rgba()` / `hsl()` / `oklch()` の色値、Tailwind の既定 palette 名（`bg-red-600`、`text-slate-500`、`border-gray-200` など `red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone` の数値段）、`black` / `white` の直書き。使うのは DESIGN.md「@theme の token 名一覧」の `--color-*` に対応する semantic utility（`bg-surface`、`text-muted-foreground`、`bg-warning text-warning-foreground` など）だけ。生の色値を書いてよいのは `web/styles.css` の `@theme` 定義の中だけ。
2. **任意値を使わない**。`p-[13px]`、`text-[15px]`、`w-[37rem]`、`gap-[6px]`、`z-[60]`、`bg-[#...]`、inline `style` の寸法・色を書かない。余白・文字・角丸・影・幅は `--spacing-*`・`--text-*`・`--radius-*`・`--shadow-*` などの token から引く。runtime の viewport 補正や safe area（`env(safe-area-inset-*)`）はデザイン値ではないので、共通部品の中に限って許す。
3. **状態色は意味で選ぶ**。success / warning / danger / info / neutral / running は背景と `-foreground` を組で使い、opacity で薄めない。色だけで伝えず日本語ラベルを付ける。API enum → ラベル → 意味色の対応は共通の対応表を通し、未知値は neutral「未確認」にする。実行終了を成功と表示しない。`danger`（淡い状態面）と `destructive`（確定ボタンの塗り）を取り替えない。
4. **状態表示を省かない**。データを取得・送信する領域は「状態表示の必須」の 6 状態をすべて持つ。
5. **target は 44×44 CSS px 以上**。「Target サイズ」の規則に従う。
6. **破壊的操作は確認を通す**。削除・却下・中止・変更破棄・常設許可・本番への昇格・整理案の適用は共通の確認 dialog を通す。dialog は対象名・影響範囲・元に戻せるか・実行後の追跡方法を示し、初期 focus は「戻る」、確定ラベルは動詞と対象を含む（「よろしいですか」「OK」だけにしない）。送信中は再送を止め、結果不明は自動再送せず再取得へ導く。一回の許可と常設許可を同じボタンにしない。破壊的操作を icon-only にしない。
7. **focus を見せる**。すべての操作に `:focus-visible` の 2px `ring` 色の outline と 2px offset を付ける。`outline: none` を代わりの表示なしに書かない。overlay は focus を中に閉じ、閉じたら trigger へ戻す。自動更新で focus を奪わない。選択と focus を同じ形で表さない。
8. **reduced motion を守る**。`prefers-reduced-motion: reduce` では drawer の移動・Skeleton の pulse・Spinner の回転・smooth scroll を止め、静止表示と状態文を残す。通常時も hover 120ms、overlay 180ms 以内。順次浮かぶカード・常時点滅・数字のカウントアップを作らない。motion ライブラリを入れない。
9. **shadcn component は Celeris token で上書きする**。shadcn の既定の外観（既定の palette、`rounded-full` の badge、影の濃い card など）を完成形にしない。部品の variant は DESIGN.md の token に対応させ、色の正本を `@theme` の 1 か所に置いて二重化しない。画面から部品の内部へ sizing や色の class を足して上書きせず、必要なら共通部品の variant を足す。依存は web-0004 で決めた範囲（shadcn・radix 系・cva / clsx / tailwind-merge と、`components.json` で固定する 1 系統の icon）に限る。ほかの UI・motion・form・font のライブラリは提案に留める。
10. **作らない形**。generic AI dashboard、card wall、巨大 hero、反復する統計カード、意味のない gradient / glass / blur、過剰余白で密度を稼がない。文字と target を縮めて密度を上げない。主表示に ID・enum・JSON を出さない（根拠・診断の詳細へ置く）。
11. **幅**。360 / 390 / 412 / 1440 px でページ全体の横溢れを作らない。flex / grid の子に `min-width: 0`、長い対象名と path は折り返す。狭い幅で失敗理由や接続状態を隠さない。
12. **外部ネットワークに出ない**。外部フォント・CDN・icon の外部配信を使わない。テストとビルドは loopback と fixture だけで完結させる。

## 状態表示の必須

取得・送信を伴う領域は、次の 6 状態を区別して表示する。shell は取得失敗でも消さない。表示の細部は DESIGN.md「状態の表示」に従う。

| 状態 | 必須の表示 | してはいけないこと |
|---|---|---|
| loading | 内容に沿った静止 Skeleton と `aria-busy`。1 秒で「読み込み中」、5 秒で遅延と再取得の手段。再取得中は既存データを残す | 進捗率の捏造、既存データを消して空白にする |
| empty | 取得成功の 0 件だけ。何が無いかの文（「判断待ちはありません」）。検索結果なしは「条件を解除」を添える | 取得失敗を empty に見せる、大きな空状態の card |
| error | 何を取得・保存できなかったか、影響、次の操作（再取得・一覧へ）。422 は該当 field に結び付ける | 理由の推測、toast だけで済ませる |
| stale | データを残したまま「更新を確認できません。最終取得 …」と「再取得」 | staleTime の経過だけで障害扱い、取得時刻を対象の更新時刻として出す |
| disconnected | shell 全幅で実際の transport 状態と最終受信。再接続後も取得成功まで stale を残す | ナビ・閲覧を止める、再接続だけで最新扱い |
| permission-denied | 403 を一般 error と分け「この操作を行う権限がありません」と戻り先。401 は保護データを消して login へ | 変更の送信、理由のない再試行ループ |

操作結果は成功・失敗・結果不明（timeout・切断）を区別し、重要な結果は対象行に残す。読み込み・成功は `role="status"`、新しい失敗は `role="alert"`。新しい画面・領域を足すときは、fixture でこの 6 状態のうち該当するものを再現できるようにする。

## Target サイズ

- button・nav・一覧の link・tab・閉じる・コピー・checkbox / radio の label は **44×44 CSS px 以上**。共通部品で `min-h-11 min-w-11`（`--spacing-11` / `--spacing-target`）を確保する。icon と checkbox の絵は 16 / 20px のままでよい。
- 独立した操作の間は 8px。不可視の hit area を隣の操作に重ねない。
- 本文中で 44px を確保できないリンクは、本文を参照名にして直後の独立したリンク行へ操作を移す。検査を通すために要素を検査対象から外さない。
- 読み取り専用の badge には target の高さは要らない。badge を pill button に見せない。
- `mobile-audit` は 360 / 390 / 412 / 1440 px で、見えている操作要素の 44×44 未満とページの横溢れを違反にする。

## 検査コマンド

`web/package.json` の scripts に実在するものだけを書く。いずれも外部ネットワークに出ない（gateway と偽 daemon は loopback、browser は host の `~/.cache/ms-playwright` の chromium）。

| 目的 | コマンド | 見るもの |
|---|---|---|
| 型 | `corepack pnpm@12.6.0 -C web typecheck` | `tsc -b` |
| lint / format | `corepack pnpm@12.6.0 -C web lint` | `biome check .` |
| 単体テスト | `corepack pnpm@12.6.0 -C web test` | vitest と `server/*.test.mjs` |
| 境界 | `corepack pnpm@12.6.0 -C web check:boundaries` | `gui/` の import 禁止、route の行数上限など |
| parity | `corepack pnpm@12.6.0 -C web check:parity` | parity 台帳の完了行と commit の対応 |
| secret | `corepack pnpm@12.6.0 -C web check:secrets` | daemon の token が build 出力・HTML・応答・ログに出ないこと（先に `build`） |
| build | `corepack pnpm@12.6.0 -C web build` | `vite build` |
| 結合・a11y | `corepack pnpm@12.6.0 -C web e2e` | Playwright。axe は専用 script を持たず e2e の中で走る |
| axe だけ | `corepack pnpm@12.6.0 -C web e2e e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts` | `axe.spec.ts`（画面ごとの critical / serious・名前・構造・focus）と `mobile-gate.spec.ts`（全画面 × 360 / 390 / 412 / 1440 で critical / serious 0・横溢れ 0）。どちらも `e2e/support/axe.ts` で `axe-core` を注入する |
| target と横溢れ | `corepack pnpm@12.6.0 -C web mobile-audit` | 4 幅の 44×44 と横溢れ。`--only "/login"` で 1 画面に絞れる |
| スクリーンショット | `corepack pnpm@12.6.0 -C web screenshots --out <dir>` | `e2e/support/screens.ts` の fixture を撮る。`--out` は必須で、run の artifacts の下を指す（リポジトリ内に置かない）。`--only <fixture>` で絞れる |

UI に触れた変更は少なくとも `typecheck`・`lint`・`test`・`check:boundaries`・`mobile-audit` と、axe を含む `e2e` を通し、変更前後の `screenshots` を 360 / 390 / 412 / 1440 px で artifacts に残す。

規則 1・2（生の色・任意値）を機械的に止める script はまだ無い。新しい script を足すまでは、変更範囲に次の grep を当てて 0 件であることを確かめる（`web/styles.css` の `@theme` は対象外）。

```sh
grep -rnE '#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?|oklch)\(|\b(bg|text|border|ring|fill|stroke|from|to|via)-(red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-[0-9]{2,3}\b|-\[[^]]+\]' web/components web/features web/routes web/lib
```

現行コードには既定 palette 名の使用が残っている（例: 状態の色付け）。それらは token 導入の工程で置き換える対象であり、新しい変更で増やさない。
