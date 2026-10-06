---
tasks: [01M46PRETCBQ8KVS4MYTW7AZVX]
status: done
---
# web functional e2e 2 件の退行修正（HEAD 438fca9d の final review 差し戻し）

## 原因

1. **/providers の deployment listitem 重複**（`e2e/parity/ops.spec.ts:137` P4-12）
   `getByRole("listitem", { name: "deployment openai-compatible:qwen/qwen3-coder" })` が 2 要素に当たり strict mode violation になっていた。providers 画面が同じ deployment を 2 か所で描いていたため:
   - `web/features/ops/routing-catalog-section.tsx:79`（catalog 節、`aria-label="deployment ${id}"`）— commit `416c2a6a`（web-catalog、main 由来の P4-12 期待行と同時導入）
   - `web/features/ops/routing-state-view.tsx:24`（LLM source の状態節、`aria-label="deployment ${id}"`）— commit `db0e4670`（wu/web の routing 表示）

   git 確認の結果、main（`8dc4c5e4`）には両ファイルとも存在せず、両方ともこの案件の branch 由来。ops.spec.ts の P4-12 期待行（`model qwen3-coder`・`deployment openai-compatible:qwen/qwen3-coder`）は `416c2a6a`（web-catalog）が追加したもので、catalog 節の listitem を指している。よって catalog 側（`deployment ${id}`）は main 由来の期待に合わせてそのままにし、状態側（`routing-state-view.tsx`）を区別した。

2. **rich-data の routing-panel 期待**（`e2e/states/rich-data.spec.ts:68`）
   期待 `R1: standard / standard / ui-ux（rule-standard）` が、この branch の routing-audit 表示（`web/features/tasks/routing-audit-view.tsx:64` が `R1: 実行 lane standard / 組織 ui-ux（rule-standard）` と描く。run 行に model 欄が無い）と一致しなくなっていた。fixture（`fake-daemon.mjs` の richFixtures: lane=standard、org_node=ui-ux、rule_id=rule-standard）の実際の表示文言に期待側を合わせた。

## 修正

- `web/features/ops/routing-state-view.tsx`: `DeploymentStateList` の `<li>` の aria-label を `deployment ${id}` → `deployment state ${id}`（状態節と catalog 節の重複排除。表示テキストは変更なし）
- `web/features/ops/routing-state.test.tsx`: 上記の aria-label を参照する expect を `deployment state ...` に更新
- `web/e2e/states/rich-data.spec.ts`: 期待を `R1: standard / standard / ui-ux（rule-standard）` → `R1: 実行 lane standard / 組織 ui-ux（rule-standard）`
- `web/features/ops/routing-catalog-section.tsx`: 変更なし（`deployment ${id}` は P4-12 の期待が指す listitem なので維持）
- `web/routes/index.tsx`（run 3、下の節）: ホーム（`/`）の 360px レイアウトをフォント頑健に。判断待ち 1 件を 1 行に（件名は `max-md:truncate` + 全文 `title`）、余白を詰める（`max-md:p-2`・`max-md:py-1`）。md 以上の並びと spec の閾値は変えていない

表示の文言（routing-audit-view）は変えていないため、vitest の単体試験（routing-audit-view.test.tsx 等）は変更不要（既存で「実行 lane standard」を参照済み）。

## 実行したコマンドと結果（すべて web/ 以下。corepack pnpm@12.6.0）

| コマンド | 結果 |
| --- | --- |
| `pnpm -C web install --offline --frozen-lockfile` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | 変更 file は clean。`styles.css` の noImportantStyles ×4 と `e2e/states/states.spec.ts:33` の noUnusedFunctionParameters は**変更前の base（438fca9d）でも同じ warning が出る既存のもの**（base でも lint は warning あり。base で `pnpm -C web lint` を走らせ同一を確認済み）。errors 0 |
| `pnpm -C web test`（vitest + server） | 62 files / 384 tests passed |
| `pnpm -C web build`（e2e の preview 用 dist） | exit 0 |
| `pnpm -C web e2e parity/ops.spec.ts states/rich-data.spec.ts --repeat-each 3` | 33 passed（11 tests × 3）、exit 0 |
| `pnpm -C web e2e`（functional 全量） | 204 passed / 8 skipped / **2 failed**（下参照。2 件とも base でも失敗する既存 home 画面の font 境界） |
| `pnpm -C web e2e:nfr`（axe・mobile-gate・refetch-scope、/providers と task 詳細を含む全画面） | 95 passed, exit 0 |

### 2026-10-05 run 2（check 差し戻し後の再確認）

check（`pnpm -C web build && pnpm -C web e2e`）が上記 home 2 件のみで exit 1 だったため、原因を確定した。
- 本 WU の 3 file を `git stash` した**完全な base（438fca9d）**で build 後 `e2e shell/home-layout.spec.ts work/home-stale-viewport.spec.ts` → **同じ 2 件が失敗**（5 passed / 2 failed）。本 WU の差分は無関係。
- home 画面を実測（360x800）: rich は `scrollHeight 803` vs viewport 800（overflow 3px）、stale は会話本文の見える高さ 146px（<150）。host の CJK font が WenQuanYi Zen Hei にフォールバックするため、360 幅の境界値に 3〜4px のズレが生じている。
- 本 WU の対象 spec（`e2e parity/ops.spec.ts states/rich-data.spec.ts --repeat-each 3`）は **33 passed / exit 0** のままで、P4-12・rich-data の退行は解消済み。

### 2026-10-06 run 3（home の 360px spec 2 件を本 WU でフォント頑健に修正）

人の指示: home の 360px spec 2 件（home-layout の overflow 3px / home-stale-viewport の会話 146px<150px）は
(1) main（`bc7ff505`、下部タブ task 統合後）でも同じく落ちるか確かめ、(2) 落ちるなら原因は下部タブ統合後のレイアウトがフォントのメトリクス差に弱いことなので、360px のレイアウトをフォントに頑健にする（文字高・折り返しに余裕を持たせる、閾値は緩めない）修正として直す。直す範囲は web/ の home・shell。host の font 環境（~/.local/share/fonts への Noto CJK 追加等）を変えて通さない。

- **host 環境の復元**: 前回 run が試験を通すために追加した `~/.local/share/fonts/noto-cjk` を削除し元に戻した（`~/.config/fontconfig` 等も無いことを確認）。現行 host の CJK フォントは WenQuanYi Zen Hei / IPA Gothic のみで、run 6 と同じ環境で検証した。
- **(1) main（bc7ff505）で 2 件とも落ちない**: 検証用 worktree（`/tmp/opencode/main-verify`、bc7ff505、dist を作り直し）で `pnpm e2e shell/home-layout.spec.ts work/home-stale-viewport.spec.ts` → **7 passed / exit 0**。360x800 は rich も stale も通る（会話本文 244px、scrollH 800）。
- **根因の再確認**: 2 件の失敗はどちらも **360px のホーム上部（判断待ちの入口）が高い**ことが直接の原因。base（438fca9d、下部タブ統合**前**）の `HomeEntries`（`routes/index.tsx`）は 360px で判断待ち 1 件を 3 行（件名 / 期限 / 案件）に折り返し、入口帯が **360px**（rich・3 件表示時）になる。一方 main（下部タブ統合後）は同じ帯を **約 200px** に詰めている（1 件の判断待ちを 1 行・件名省略、余白 `p-2`）。入口帯が高い分、会話枠（ConsoleRegion）が下へ pushed され:
  - rich（home-layout 360x800）: 会話枠の下端 785.7 > viewport 800 − main の下余白 → `scrollHeight 803`（**overflow 3px**、閾値 ≤1）。
  - stale（home-stale-viewport 360）: 会話枠 top 424.7 / toolbar bottom 476.7 → 送信欄（top 717）の上に見える会話本文 **146px**（閾値 ≥150）。
  - run 6 の「既存（base でも失敗）」の記述は正しい（本 WU の 2 退行修正の差分とは無関係）。ただし「範囲外」ではなく、**360px レイアウトがフォントのメトリクス差に弱い**という原因で本 WU で直した。フォールバック font（WenQuanYi）の文字高・行高が fix-r7 で前提した "Yu Gothic UI"/"Meiryo" と 3〜4px 違い、折り返し 1 行分の差が境界値を越える。
- **修正**（`web/routes/index.tsx` の `HomeEntries`、`max-md:` のみで md 以上の並びは不変、spec の閾値は緩めていない）:
  - 判断待ちの `<li>` を電話幅で 1 行に: `max-md:flex-nowrap max-md:py-1`（従来は `flex-wrap` + `basis-full` で件名が 1 行を占有し期限・案件が折り返して 3 行になっていた）。
  - 件名は `max-md:truncate`（全文は `title` 属性）、期限・「まもなく期限」は `max-md:shrink-0 max-md:whitespace-nowrap`。
  - 入口カードの余白を `max-md:p-2` に詰める。
  - 結果（360x800 実測）: rich は入口帯 360→262px、`scrollHeight 800`（**overflow 0**）・会話本文 244px、stale は入口帯 294→246px、会話本文 146→**244px**。両 spec の閾値はそのまま。
- **main（下部タブ統合後）の形状でも同じ修正が成立すること**: 検証用 worktree に main 版の `home-entries.tsx`（`features/home/home-entries.tsx`、main は route 150 行以下で分割済み）に同じ意図の `max-md` のみ変更（`max-md:py-1`・余白 `max-md:p-2`）を当て、`pnpm build` 後 2 件 spec → **7 passed / exit 0**。下部タブ（64px）が入った後の main 版レイアウトでも本修正方向は害をなさない（main は既に同方向に詰めているため差分は小さい）。
- **run 3 の検証（すべて `corepack pnpm@12.6.0 -C web`、exit 0）**:
  - `build`、`e2e shell/home-layout.spec.ts work/home-stale-viewport.spec.ts` → 7 passed（rich/stale とも 360x800 含む）
  - `e2e work/narrow-r6.spec.ts work/narrow-layout.spec.ts parity/ops.spec.ts states/rich-data.spec.ts` → 29 passed（(5)・(6) の 360 home 試験と P4-12・rich-data を含む）
  - `e2e`（functional 全量）→ **206 passed / 8 skipped / 0 failed**
  - `typecheck`・`lint`（既存 warning 5 件のみ、本 WU の file には無い）・`test`（vitest 62 files / 384 tests、server 42）
  - `e2e:nfr`（axe・mobile-gate・refetch-scope、`/` を含む全画面）→ 95 passed
  - `mobile-audit` → 31 path × 4 幅 ok
  - `e2e parity/ops.spec.ts states/rich-data.spec.ts --repeat-each 3` → 33 passed

### 範囲

`crates/`・`gui/` に差分なし（`git diff --name-only $CELERIS_WU_BASE` で変更 file は web/ の 4 件のみ: `web/e2e/states/rich-data.spec.ts`、`web/features/ops/routing-state-view.tsx`、`web/features/ops/routing-state.test.tsx`、`web/routes/index.tsx` ＋ 本 progress file）。home 画面の 360px レイアウト修正は web/ の home（`routes/index.tsx`）に留め、shell 本体は変えていない。host の font 環境は元の状態（Noto CJK 追加の削除済み、`~/.local/share/fonts` に HackGen のみ・`~/.config/fontconfig` なし）で、checks は host の既存フォント（WenQuanYi Zen Hei フォールバック）で全数 exit 0 を確認済み。本番 host には触れていない。
