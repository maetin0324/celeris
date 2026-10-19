---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: campaign 公開契約の明示変換と固定証拠受渡し（campaign-bridge）
work_unit: campaign-bridge
status: done
updated: 2026-10-19
---
# campaign-bridge

変更は `crates/campaign` のみ（task-core は不変、逆依存なし）。

## 内容
- `variant.rs`: `From<&VariantRef> for CampaignVariantRef`、`TryFrom<&CampaignVariantRef> for VariantRef`（未知 id は `VariantError::Schema`）、`OrgPin` ⇄ `CampaignOrgPin`。`effective_profile_sha256` は task-core 版へ委譲（二重実装を削除）。`VariantRef`/`VariantSpec`/manifest/lock の struct・serde 形・hash は不変。
- `celeris_exec.rs`: `with_variant_org`。variant があるのに pin が無ければ投入前に拒否。POST は `campaign.variant` と `campaign.variant_org` を task-core 型で送る。GET 照合は旧 `task.campaign.variant` を捨て、`detail.campaign`（次段 ops-campaign-view の `TaskDetail.campaign`）を読む。`variant`・`variant_org` の欠落/変更は `Refused`、投入前の再照会（submit）は `campaign_id`/`case_id`/`arm`/`repetition`/`manifest_hash`/`routing_setting` も POST した値と照合。自分で作った task の応答が不一致なら閉じる。
- `main.rs`: manifest と同 dir の封印済み `variant-spec.json` から org pin を取り、参照が manifest と一致しなければ拒否。

## 試験（fake Transport。実 LLM・本番 API なし）
- `variant_submit.rs` 12 件（新規 5: 変換の JSON 形一致・往復・固定 selection hash `b6132a44…64bc`、未知 id 拒否、profile hash 同値＋固定値 `066bd493…f071`、POST の variant_org、欠落/変更 9 通りの Refused）、`ablation_contract.rs` 17 件（fake を新 GET 形へ更新）。

## 証拠
- `cargo test -p celeris-campaign` → 全 test binary ok（失敗 0）
- `cargo test -p celeris-campaign --test variant_submit --test ablation_contract` → 12 passed / 17 passed
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `--all-targets` の clippy は未変更の `tests/env_lock.rs:529`・`tests/launcher.rs:603` で既存の指摘（本葉の範囲外。ゲートは `--all-targets` 無し）
- `bash scripts/dev/test-parallel.sh` は統合葉に委ねた（本葉では回していない）

## 未解決・注意
- 実 API は `TaskDetail.campaign` 未提供（ops-campaign-view/api-atomic で追加）。それまで実 daemon に対する variant 付き照合は fail-closed で Refused になる。
- P1（`variant-b0.json` の承認済み sha256 明示）は本葉では未実施（後段）。
