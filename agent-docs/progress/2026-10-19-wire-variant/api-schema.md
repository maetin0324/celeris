---
wu: api-schema
completed: 2026-10-19
---
# api-schema

`UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` で docs/api/v1/api-v1.schema.json を再生成（+11 行: `TaskDetail.campaign` = CampaignIdentity | null、下位型は core-api-schema で生成済み）。生成源 Rust は未変更。
`UPDATE_SCHEMA=1 cargo test -p task-core --lib`（1032 passed）で event.schema.json は差分なし。docs/protocol は RunContext 未変更のため差分なし。web 生成型は次の web-types が担当。

証拠: UPDATE_SCHEMA 無しで `committed_schema_matches_generated` 1 passed。差分は docs/api/v1/api-v1.schema.json のみ。
