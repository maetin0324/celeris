---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 下位 wire 型と不変な実行 pin（core-wire）
work_unit: core-wire
status: done
updated: 2026-10-19
---
# core-wire

- 追加: `crates/task-core/src/variant_wire.rs`（`CampaignVariantRef` / `CampaignOrgPin` / `effective_profile_sha256`）。`lib.rs` に `pub mod variant_wire;`。
- JSON 名は campaign `VariantRef` / `OrgPin` と同一（`deny_unknown_fields`、省略なし）。task-core は hash を再計算せず、形（非空・64 桁小文字 hex・nodes 重複なし・coding_node ∈ nodes）だけ検証。
- `CampaignIdentity` へは**欄を追加していない**（依頼どおり加算的公開のみ）。よって struct literal・schema・web 生成型は不変。campaign への依存は task-core の Cargo.toml に無い。
- 試験（5 件）: wire roundtrip / 未知欄・欠落欄拒否 / 不正 hash / org pin 検証 / profile hash が node_id・chain を無視。
- 未解決: campaign 版 `variant::effective_profile_sha256` との同値試験は campaign 側なので campaign-bridge 葉で固定する（実装は同一手順: node_id・chain を空にし canonical JSON の sha256）。
