---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: worker run pinの型・証拠保存・再開検査（worker-pin）
work_unit: worker-pin
status: done
updated: 2026-10-19
---
# worker-pin

## 変更（task-worker のみ）
- `protocol/campaign.rs`: `CampaignRunPin { identity, org_nodes_sha256, selected: CampaignSelected{harness_id,source_id,model_id}, daemon_instance_id, config_sha256 }`（`deny_unknown_fields`、`validate`）。`RunContext.campaign: Option<CampaignRunPin>`（serde default・skip）。plain な run は JSON もプロンプトも不変。
- `campaign_pin.rs`: `enforce(run_dir, req)`。pin 有り → 形を検査（`campaign_pin_invalid`）、task の保存 pin（`<artifacts_dir>/campaign-pin.json`）と全欄一致を要求（`campaign_pin_mismatch`）、無ければ保存、run 証拠 `runs/<run_id>/campaign-pin.json` を書く。pin 無しで保存 pin がある後続 run（continuation・retry・再開）は `campaign_pin_missing`。pin も保存も無い plain run は何も書かない。
- 全 adapter の run 開始点（claude_code・codex・acp・aider・pi・langmem・paperqa・local_deep_research・subprocess）で `write_run_request` の直前に呼ぶ。失敗は `AdapterError::Other`（spawn 前）。
- worker 所有の `RunContext` literal は全て `..Default` 系で、壊れた literal は無かった。外部 crate の literal は後続葉（ctl/api/celeris-worker-callsite）。

## 試験（`campaign_pin_tests.rs`、名前に `variant` を含む 6 件）
正: 証拠保存と同一 pin での再開、plain の JSON 省略と往復。負: 不一致（identity・config hash）、pin 欠落、不正な pin、spawn 前の拒否（`run_subprocess`）。

## 証拠
- `cargo test -p task-worker variant` → 6 passed、0 failed
- `cargo fmt --all -- --check` → exit 0、`cargo clippy -p task-worker --tests -- -D warnings` → exit 0
- `cargo test -p task-worker --lib` → 1031 passed、66 failed。失敗は browser 系 65（worker sandbox が userns 不可。本変更と無関係）と `protocol::tests::committed_schema_matches_generated`（RunContext 変更による protocol schema の drift。protocol-schema 葉が再生成する。想定どおり）

## 未解決事項
- dispatcher が `RunContext.campaign` を埋める配線は dispatch-worker-pin。protocol schema 再生成は protocol-schema。
- daemon 再起動で `daemon_instance_id` が変わると、保存 pin のある task の再開は mismatch で失敗する（専用環境は再起動しない運用）。
