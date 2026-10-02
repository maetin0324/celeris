# web ADR-W4: web/ の design system（DESIGN.md の token・pattern）と shadcn の導入

---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

- Date: 2026-10-02
- Status: Accepted（文書の決定。依存の導入と token の実装は後続の foundation の工程で行い、本 ADR の時点では web/ のコードを変えない）
- 関連: ADR-0081 D7（Tailwind CSS 4 と shadcn/ui の採用・表示契約）、docs/frontend/DESIGN.md、docs/frontend/FRONTEND_CONTRACT.md、
  docs/frontend/UX_AUDIT.md（cosmetic と IA/component 層の切り分け・4 群の割り当て・quality gate critique）、web UI/UX 改善の人の回答 h2（依存方針）

## 文脈

web/（ADR-0081 の SPA）は Phase 1〜6 で機能の parity を満たしたが、見た目と部品は最小限のままである。
`web/styles.css` は `@import "tailwindcss";` だけで `@theme` が無く、`web/components/ui/` は `button.tsx` と `panel.tsx` の 2 つ。
Button は `border-neutral-400`・`outline-blue-700` のような Tailwind の生の色を直接持ち、主・副・危険の区別が無い。
UX_AUDIT.md は、確認なしの破壊的操作（`window.confirm` や確認なしの「却下」「適用」）、同じ見た目で並ぶ「確認」「ログイン開始」「削除」、
`h2` だけが節の境目になる縦積み、JSON の `pre` をそのまま出す画面を挙げ、画面ごとの手直しでは揃わないと判断した。

ADR-0081 D7 は「shadcn/ui の Base UI 系部品」を採用するとしたが、依存を足す葉は未実施で、どの primitive を基礎にするかと
見た目をどう Celeris に合わせるかは決まっていなかった。web UI/UX 改善の計画で人に依存方針（h2）を尋ね、
「shadcn を導入する。見た目は Celeris の token で上書きする」と回答を得た。

## 決定

### D1. docs/frontend/DESIGN.md の token・pattern を web/ の design system とする
- 色・文字・余白・角丸・影・幅の値は DESIGN.md「@theme の token 名一覧」の名前（`--color-*`・`--font-*`・`--text-*`・`--spacing-*`・`--radius-*`・`--shadow-*`・`--breakpoint-*`）を正本とし、
  `web/styles.css` の Tailwind v4 `@theme` に集約する。feature 側は semantic utility（`bg-surface`・`text-danger-foreground` 等）と共通部品の variant だけを使う。
- 状態色 6 組（success / warning / danger / info / neutral / running）、Button・Badge・Table/list・Drawer・code/log surface、
  loading / empty / error / stale / disconnected / permission-denied、破壊的操作の確認、focus、reduced motion、44×44 px target、
  360 / 390 / 412 / desktop の幅ごとの規則は DESIGN.md の節に従う。dark mode は DESIGN.md のとおり提案に留め、本 ADR では採らない。
- 対象は「高密度の ops workbench」。generic AI dashboard・card wall・巨大 hero・意味のない gradient/glass・過剰余白を避け、unstyled admin にも戻さない（理由は DESIGN.md「原則」）。

### D2. docs/frontend/FRONTEND_CONTRACT.md を agent の規則とする
- 将来の agent（と人）が web/ の画面・部品を変えるときは FRONTEND_CONTRACT.md の規則（token 以外の生の色・任意値の禁止、状態表示の必須、target サイズ等）を守り、
  そこに書かれた検査コマンド（`corepack pnpm@12.6.0 -C web …` の typecheck / test / screenshots / mobile-audit / e2e の a11y）で確かめる。
- DESIGN.md と FRONTEND_CONTRACT.md が食い違ったら、値・見た目は DESIGN.md、手順・検査は FRONTEND_CONTRACT.md を正とし、同じ変更で両方を揃える。

### D3. ADR-0081 D7 の「Base UI 系」を「Radix 系」に改める
- D7 のうち部品の基礎だけを本 ADR で上書きする（h2 の回答で依存に radix 系を含めたため）。D7 の他の表示契約（skeleton、遅延表示、44×44 px、4 幅の gate、focus 復帰、IME）は変えない。

## 依存方針

h2 の人の回答（**shadcn を導入し、見た目は Celeris token で上書きする**）をそのまま記録する。

- **shadcn を導入する**。shadcn は「部品の source を `web/components/ui/` に写して持つ」方式で、実行時の依存は部品が使う primitive と小さな utility だけになる。
  写した source は Celeris のコードとして扱い、レビュー・変更してよい。
- **`components.json`** を `web/` に置く。style は Radix 系、`tailwind.config` は空（Tailwind v4 の CSS-first）、`css` は `styles.css`、
  alias は既存の import 規約（`web/components/ui`、`web/lib`）に合わせる。`iconLibrary` は D-icon の決定を書く。
- 入れる依存（exact pin、`web/pnpm-lock.yaml` を更新して commit する）:
  - **radix 系**: `radix-ui`（統合 package）または必要な `@radix-ui/*` の個別 package。どちらにするかは導入の葉で shadcn CLI の現行の出力に合わせて 1 つに決め、混在させない。
  - **`class-variance-authority`（cva）**: Button・Badge 等の variant 定義。
  - **`clsx`** と **`tailwind-merge`**: `cn()` utility（`web/lib/utils.ts` 相当）で class を結合し、衝突を解決する。
- **見た目は Celeris token で上書きする**。shadcn の既定の変数（`--background`・`--primary`・`--destructive` 等）は DESIGN.md の `--color-*` へ `@theme inline` で対応させ、
  色の正本を二重化しない。shadcn 既定の配色（neutral/zinc 系）・角丸・影をそのまま残さない。
  shadcn の `destructive`（確定操作の塗り）と Celeris の `danger`（淡い状態面）は別の用途として両方を持つ（DESIGN.md「@theme の token 名一覧」）。
- **icon 方針（D-icon）**: 1 系統の線画 icon に統一し、`components.json` の `iconLibrary` で固定する（shadcn の既定は lucide で、その場合は `lucide-react`）。
  icon library の追加は h2 で列挙された依存の外であるため、導入の葉で人の承認範囲を確認してから入れる。承認が無い間は icon を使わず可視ラベルで表す（現行どおり。DESIGN.md の主要操作は可視ラベル必須なので機能は欠けない）。
  別の icon set・外部配信（CDN・web font の icon）を混ぜない。寸法 16/20px・stroke 2・`currentColor`・装飾 icon は `aria-hidden`、icon-only 操作は accessible name と 44px target を持つ（DESIGN.md「Icon」）。
- 入れないもの: 上の列挙以外の UI・motion・form・theme の依存（例えば motion 系、form 系、theme 切替系、Web font 配信）。必要になったら PROGRESS の提案か新しい web ADR で扱う。
  ビルド・テストは外部ネットワークに出ない（registry からの取得は導入の葉の `pnpm install` だけ）。

## 却下案

- **自作 primitive のみ（現行の延長）**: Dialog の focus trap・focus 復帰・Escape、Sheet/Drawer、Dropdown、Tooltip、Tabs のキーボード操作と ARIA を自前で正しく作り続ける費用が大きく、
  UX_AUDIT の破壊的操作の確認を揃える工程が primitive の作り込みで止まる。h2 の回答とも合わない。
- **他の component library（MUI・Chakra・Mantine・Ant Design 等の完成品 library）**: 独自の theme 機構と runtime style を持ち込み、Tailwind v4 の `@theme` と token の正本が二重になる。
  bundle も大きく、高密度の表・一覧の見た目を合わせるには上書きが多い。部品の source を手元に持てないため、Celeris の状態語彙に合わせた変更がしにくい。
- **Base UI 系の shadcn（ADR-0081 D7 の当初案）**: h2 の回答が radix 系を指定した。shadcn の部品例・skill の参照・周辺の知見が Radix 系に厚く、導入と保守の手順を揃えやすい。
- **shadcn 既定の見た目のまま使う**: generic な admin/AI dashboard の見た目になり、状態色 6 組・danger と destructive の区別・日本語の文字組・44px target といった
  DESIGN.md の規則を満たさない。「既定の外観を完成形にしない」は DESIGN.md「原則」の方針でもある。
- **Tailwind の生の色・任意値で画面ごとに整える**: 現行 Button のように値が部品と画面に散り、状態色の contrast 比を一箇所で保証できない。FRONTEND_CONTRACT.md で禁止する。

## 帰結

- **依存の増加**: `web/package.json` に radix 系・`class-variance-authority`・`clsx`・`tailwind-merge`（承認された場合は icon library）が加わり、`components.json` と
  `web/components/ui/` の shadcn 由来の source が増える。いずれも exact pin で、selfdeploy の web/ の段（web ADR-W3 D3）の `pnpm install` 対象が増える。
- **更新手順**: shadcn の部品は写した source なので自動では更新されない。更新は (1) 依存の版を pin で上げ lockfile を commit、(2) 必要な部品だけ shadcn CLI の差分を見て手で取り込み、
  Celeris token の上書きと variant を保つ、(3) 下の検査を通す、の順で 1 commit にまとめる。既定の見た目へ戻す差分は取り込まない。
- **検査**: 変更ごとに FRONTEND_CONTRACT.md の検査（`corepack pnpm@12.6.0 -C web typecheck` / `test` / `screenshots`（360/390/412/desktop）/ `mobile-audit` / `e2e` の axe）を通す。
  生の色・任意値の混入は FRONTEND_CONTRACT.md の規則でレビュー時に拒否する（機械検査の追加は後続の提案）。
- **既存コードへの影響**: `web/components/ui/button.tsx`・`panel.tsx` は shadcn の Button と Celeris の Panel 相当に置き換わり、feature 側の生の色の class は token の utility へ移る。
  移行は UX_AUDIT の 4 群の割り当て（foundation → 画面群）に沿って段階的に行い、各段で 4 幅のスクリーンショットを比べる。
- ADR-0081 D7 の「Base UI 系」の記述は本 ADR の D3 で上書きされた（ADR-0081 本文は書き換えない）。
