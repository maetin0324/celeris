---
wu: core-web-types
completed: 2026-10-19
---
# core-web-types

web/package.json は packageManager pnpm@12.6.0、`web/scripts/gen-types.mjs` は docs/api/v1/api-v1.schema.json から web/api/generated/{schema.json,types.ts} を生成。gui/ に生成型の出力先（gen:types）は現 tree に存在しない（gui/ は app・scripts・test のみ）。

手順: `corepack pnpm@12.6.0 -C web install --offline` → `corepack pnpm@12.6.0 -C web gen:types`。差分は web/api/generated の 2 file のみ（+86 行、CampaignIdentity の variant 系型）。

証拠: install exit 0、gen:types exit 0、`pnpm exec tsc -p tsconfig.app.json --noEmit` が出力なしで成功。
