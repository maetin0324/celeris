---
wu: core-api-schema
completed: 2026-10-19
---
# core-api-schema

`UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` で docs/api/v1/api-v1.schema.json を再生成（+71 行: CampaignIdentity の variant / variant_org、CampaignVariantRef、CampaignOrgPin 等）。docs/protocol は RunContext 未変更のため差分なし。web 生成型は次の core-web-types が担当。

証拠: 再生成後に UPDATE_SCHEMA 無しで同試験 1 passed（0 件一致ではない）。
