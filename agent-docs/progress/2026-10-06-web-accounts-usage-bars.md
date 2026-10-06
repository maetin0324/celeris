---
title: web アカウント画面の 5 時間枠・7 日枠の残量バー
tasks: [01M46VEZYDYQE0NGWSSWDJCZRY]
status: done
updated: 2026-10-05
---

# web アカウント画面の 5 時間枠・7 日枠の残量バー

完了日: 2026-10-05。旧 GUI（gui/app/routes/accounts.tsx の UsageBar）と同等の表示を web/ に足した。gui/・shell・crates は無変更。

## 何を変えたか

- `web/features/ops/accounts-screen.tsx`
  - `UsageBar`: 各アカウントに「短期枠（5時間）」「長期枠（7日）」の 2 本。`role="meter"`・`aria-valuemin/max/now`・`aria-valuetext`（使用・残り・段階・リセットまで）。文字で「使用 N% / 残り M%」と、注意（70% 以上）・上限間近（90% 以上）の語を出す（色だけに頼らない。境目は旧 GUI の usageTone と同じ）。
  - 塗りは token（`bg-primary` / `bg-warning-foreground` / `bg-danger-foreground`、地は `bg-muted`）。素の `<meter>` は塗りを token で揃えられないため div + role（biome の useSemanticElements は理由付きで抑止）。
  - リセットまでの残り時間は取得時刻（`query.dataUpdatedAt` + server 時計のずれ）基準で、上位 2 単位（「2時間15分」「3日4時間」）と絶対時刻。窓が無いときは「-（観測なし）」。
  - DataList に「使用量の観測」（observed_at の絶対・相対時刻・source・status）と「選択の score」（score か「除外: <日本語>（<code>）」）を追加。状態欄の除外理由も日本語ラベル化（five_hour_exhausted → 短期枠を使い切りました 等、旧 GUI と同じ語）。
- `web/features/ops/accounts-screen.test.tsx`: 使用率 0%・中間（45%/75%）・100%＋five_hour_exhausted 除外・窓なし/usage なしの fixture で、表示値・幅・段階（data-usage-tone と塗りの class）・aria・残り時間・除外理由を確かめる 5 件を追加（計 8 件）。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0 |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 59 files / 361 tests、server node:test 42 pass） |
| `corepack pnpm@12.6.0 -C web e2e`（functional） | exit 1: 204 passed / 2 failed / 8 skipped。失敗は `e2e/shell/home-layout.spec.ts:26`（360x800）と `e2e/work/home-stale-viewport.spec.ts:15`（150px 期待に 146px）。accounts-screen を HEAD（8dc4c5e4）版に戻して build し直しても同じ 2 件が同じ値で落ちる（既存の失敗、ホーム／shell 側） |
| `bash scripts/dev/test-parallel.sh` | exit 0（nextest passed 3942 / failed 0 / ignored 13） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |

Screenshot（run の成果物ディレクトリ `before/`・`after/`、360/390/412/1440 幅、fixture 5 アカウント: 0%・中間・100% 除外・窓なし・usage なし）。撮影台本は同ディレクトリの `shot-accounts.mjs`（/api/accounts を page.route で差し替え。repo には置いていない）。

## 未解決事項

- web/ には dark の token がまだ無い（`styles.css` は light のみ。preferences の theme は保存だけ）。今回の部品は token だけで塗るので、dark の token が入れば追従する。`colorScheme: dark` で撮った after/accounts-dark-* は light と同じ見え方。
- 上記 home 系 e2e 2 件の失敗は本変更と無関係で、並行中の下部タブ・縦の長さの task（01M46VAZ0G）の範囲と思われる。

## 提案

- `formatRemaining`（残り時間の上位 2 単位表記）は他画面（providers の cooldown 等）でも使えるので、必要になったら `web/lib/time.ts` へ移す。
