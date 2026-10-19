---
wu: protocol-schema
status: done
completed: 2026-10-19
---
# protocol-schema
- `UPDATE_SCHEMA=1 cargo test -p task-worker committed_schema_matches_generated` で docs/protocol/worker-protocol.schema.json を再生成（+180 行: RunContext.campaign と CampaignIdentity / CampaignVariantRef 等）。UPDATE_SCHEMA なしで再検査 1 passed。
- docs/api と web の生成型に RunContext / worker protocol 型は含まれない（grep 0 件）。API 側の別生成葉は不要。
- celerisctl: `cargo check -p celerisctl --all-targets` exit 0。RunContext literal は default 展開で同期済み、celerisctl のコード変更不要（ctl-worker-callsite は計画から外した）。
