---
wu: web-types
completed: 2026-10-19
---
# web-types

api-schema 後の docs/api/v1/api-v1.schema.json から web 生成型を再生成。

- `corepack pnpm@12.6.0 -C web install --offline` exit 0
- `corepack pnpm@12.6.0 -C web gen:types` exit 0。差分は web/api/generated/{schema.json,types.ts} のみ（TaskDetail.campaign?: CampaignIdentity | null、+12 行）
- `corepack pnpm@12.6.0 -C web typecheck`（tsc -b）exit 0、全利用者の型検査成功
- schema は読取のみ。gui/ に生成先なし。
