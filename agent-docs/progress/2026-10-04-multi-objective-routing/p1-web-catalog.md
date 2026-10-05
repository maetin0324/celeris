---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
status: done
completed: 2026-10-05
---
# Phase 1: web/ の routing catalog 表示

ADR 2026-10-04-multi-objective-model-routing §9・§10 Phase 1 の web/ 側。

- 生成型: `node web/scripts/gen-types.mjs` で `web/api/generated/{types.ts,schema.json}` を更新（`RoutingCatalogView`・`CatalogModelView`・`CatalogDeploymentView` ほか、routing の optional trace 型）。
- 画面: providers 画面（`/providers`）の「LLM source」節の後に「モデルの catalog」節を足した（`web/features/ops/routing-catalog-section.tsx`、表示用の関数は `routing-catalog.ts`）。model（能力・context・品質・価格）と deployment（source × model、上流 model 名・課金・lane・価格）を別の一覧で出す。deployment の価格は上書き → model の価格 → 不明の順で、出所を併記する。
- 欠測: 価格・品質・能力・context の null は「不明」と書き、価格は「0 円ではありません」、品質は「品質は保証されません」と添える。明示の 0 は既知の値として `$0` を出す。catalog に無い model を指す deployment は能力・品質を作らず注記する。
- 色: web/ には gui の `--*-soft-fg` token が無い。「不明」と警告は既存の濃色 `text-amber-900`（白地で 7:1 超）を使った。
- fixture: `web/e2e/support/fake-daemon.mjs` に `routingCatalogFixture`（旧設定由来の形、Qwen 側は全欄欠測）を足し、`/api/v1/llm/routing/catalog` で返す。e2e `parity: /providers` に catalog の表示確認を足した。
- event 種は増えないので `EVENT_KINDS` / `EVENT_INVALIDATION` は変えていない。query key は `accountKeys.list({ section: "routing-catalog" })`（llm sources と同じ domain）。

## 証拠

- `pnpm -C web typecheck`（`tsc -b`）: exit 0
- `pnpm -C web exec vitest run`: 189 件成功（exit 0）
- `pnpm -C web exec vitest run -t routing_catalog_missing_metadata`: 1 件成功（fixture の schema 検証・model/deployment の区別・不明表示・$0 が出ないことを assert）
- `pnpm -C web exec playwright test e2e/parity/ops.spec.ts -g providers`: 2 件成功
- `biome check`（触ったファイルのみ）: 指摘なし

Rust は変更していない。workspace の `test-parallel.sh` と clippy は close 葉で流す。

## 未解決事項

- policies の詳細（重み・制約）は画面に出していない（Phase 1 は mode と model/deployment の区別が範囲）。

## 提案

- web/styles.css に gui と同じ `--*-soft-fg` token を入れると contrast 規則を両 frontend で同じ名前で書ける。
