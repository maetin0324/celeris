---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
status: done
completed: 2026-10-05
---
# Phase 1: gui/ の routing catalog 表示

ADR 2026-10-04-multi-objective-model-routing §9・§10 Phase 1 の GUI 側。`pnpm gen:types` で `gui/app/celeris/types.ts` に
`RoutingCatalogView`・`CatalogModelView`・`CatalogDeploymentView`・`RoutingPolicy`・`RoutingTraceV1` などを生成した（手編集なし）。

- `/providers` の loader が `GET /llm/routing/catalog` も読む。409 `llm_proxy_unavailable` と取得失敗は `routingCatalog: null` にし、画面は落とさない（`/llm/sources` と同じ扱い）。
- 新しい節「model routing catalog」（`RoutingCatalogOverview`）は model と deployment を別のカード群に出す。model には単価・品質・context・tools、deployment には LLM source（`source_ref` の原文）・model・課金種別・lane・単価の上書きを出す。
- 整形は `gui/app/lib/routing-catalog.ts` の純関数だけ。欠測（`null`・空配列）は「不明」。0 は celeris が 0 と返したときだけ `$0` と出す。deployment の単価の上書きが無いときは「上書きなし（model の単価に従う）」とし、GUI で値を写したり合算したりしない。
- fixture: `gui/test/mock-celeris/fixtures.ts` の `routingCatalogView()`（型付き）と、`gui/scripts/lib/celeris-fixture.mjs` の `GET /api/v1/llm/routing/catalog`・`GET /api/v1/providers`。`ROUTES` に `providers` を加え、mobile-audit・e2e:mock の対象にした。
- `/providers` が初めて監査対象になり、既存の「アカウント画面」リンク（LLM source 節の説明文）が tap-target（355×37）で違反した。そのリンクに `inline-flex min-h-11 items-center` を足して直した。
- biome は触ったファイルだけに掛けた（`biome check --write <6 files>`）。

## 証拠

- `pnpm gen:types && git diff --exit-code app/celeris/types.ts`（commit 後の再生成）: 差分ゼロ
- `pnpm typecheck`: exit 0
- `pnpm test`: 92 files / 1306 tests passed
- `pnpm exec vitest run -t routing_catalog_missing_metadata`: 3 passed（`test/unit/routing-catalog.test.tsx` 2 件・`test/unit/providers.test.ts` 1 件）
- `pnpm mobile-audit --routes providers`: violations=0（light/dark）
- `pnpm e2e:mock`: failures []
- `pnpm exec biome check <触った 6 files>`: No fixes applied

## 未解決事項

- policies（lane ごとの重み・制約）は型だけ生成し、画面には出していない（Phase 1 の受け入れ条件は model/deployment の区別と欠測表示）。
- task 詳細の routing 監査（`optimizer` trace）の表示は、この葉の範囲外。

## 提案

- `source_ref` は `openai_compatible:<id>`（下線）で、`/llm/sources` の `openai-compatible:<id>`（hyphen）と形が違う。API 側で同じ形に揃えるか、対応表を返すと GUI で突き合わせられる。
