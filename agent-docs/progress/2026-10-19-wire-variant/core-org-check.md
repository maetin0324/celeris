---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 共有org照合と全node digest・統合ビルド境界の修復（core-org-check）
work_unit: core-org-check
status: done
updated: 2026-10-19
---
# core-org-check

## 統合ビルド境界（event 670: variant_wire E0432/E0433）
- 現 HEAD で `crates/task-core/src/lib.rs` に `pub mod variant_wire;`、campaign は task-core に path 依存。`cargo tree -p celeris-campaign` で task-core が解決され、`cargo build -p celeris-campaign` は成功。
- 欠落ではなかったので task-core の公開は変更していない（cache 削除・環境上書きなし）。event 670 の統合 HEAD 自体は読取経路で確認できていない（原因は統合時点の古い target か一時状態と推測、未確証）。

## 追加（task-core `variant_wire.rs` のみ）
- `org_effective_digest(&[OrgNode])`: 全 node の実効 profile（node_id・chain 込み）を id 順に並べた canonical sha256。入力順に不変、空/重複 id は拒否。
- `CampaignOrgPin::check_org(&[OrgNode])`: node 集合（順序無関係）・coding node・その実効 profile hash を照合。欠落/未知/重複 node と profile drift を理由付きで拒否。
- 既存の profile hash・wire 型は不変（`existing_profile_hash_unchanged`）。

## 証拠
- `cargo test -p task-core variant_wire` → 9 passed（新規 4）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` は統合葉に委ねた（未実行）
