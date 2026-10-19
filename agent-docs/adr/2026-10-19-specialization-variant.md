---
tasks: [01M59P79S6NTGRN3X7SQDV9C0G]
---
# ADR: 専門分担 variant の境界契約（M1-specialization）

- Date: 2026-10-19
- Status: Accepted
- 決定者: 計画（WU baseline-contract）。実装は後続 WU

## Context

`crates/campaign/fixtures/ablation/selection.json`（`selection.sha256` で固定、変更しない）は機構として `specialization` を選んだ。比較は spec-on / spec-off の 2 variant。切替点は org だけで、新しい task-template・profile 欄・scheduler・router は作らない。詳細な照合と型は `agent-docs/progress/2026-10-19-wire-variant/contract.md`。

## Decision

- D1: `M1-spec-on` = B0 組織。`M1-spec-off` = 根 `cos` と単一の非根作業課。作業課の `EffectiveProfile`（`node_id`・`chain` を除く）は B0 の coding 課と同値で、`effective_profile_sha256` の一致を条件にする。
- D2: 共有型の所有は `campaign` crate（`variant.rs`: `VariantId`・`VariantSpec`・`NonTargetPin`・`VariantRef`）。task-core/API は `CampaignIdentity.variant: Option<VariantRef>` を運び比較するだけ。
- D3: `VariantSpec` は内容 hash（`manifest::compute_sha256` と同じ canonical 手順）を持ち、`selection_sha256` を `selection.sha256` と照合する。manifest と lock の `variant` 欄は省略可で、省略時の hash・挙動は従来どおり。
- D4: 非対象（全役割モデル/harness・資料・KB snapshot・予算・安全/承認上限・grader）の差分、hash 不一致、variant の欠落、再開・冪等照会での不一致は拒否し、適用済み扱いにしない。再 seal での variant 差し替えも拒否する。
- D5: 延期の review-off / KB-off は扱わない。

## Consequences

次段は task-core/API（`CampaignIdentity.variant`、409 比較）、`CelerisExecutor` の POST/GET 照合、`ablation_contract.rs` の拒否試験を足す。旧い仕組みの置換はなく、削除対象はない。

## 付記（campaign-pin: manifest/lock/preflight への固定）

- `CampaignManifest.variant` と `ExecutionPin.variant`（lock）は `Option<VariantRef>`、省略時は直列化しない（従来の manifest/lock hash は不変）。manifest の `validate` は参照の selection hash を同梱 `selection.sha256` と照合する。`bind_to_manifest` は `ExecutionPin` の等値比較で参照の食い違いを拒否し、`check_successor` は variant 参照の差し替え・脱落を後継として認めない。
- 基準は `variant_pin::baseline_pin`（B0 の manifest・lock・`GET /api/v1/org` 応答 + 全役割モデル）から VariantSpec と独立に組む。`OrgSnapshot::from_get_org` は `items` と `effective_profiles` が揃って一致しなければ拒否する（新 endpoint なし）。
- preflight（`preflight::check_variant`）は manifest に参照があるときだけ `variant-spec.json`・`variant-b0.json`（manifest と同じ dir）と稼働 daemon の `GET /api/v1/org` を `variant_pin::check_pin` に通す。欠落は拒否。参照が無ければ従来どおり。POST/GET Transport は次の葉。

## 付記（verify-contract: 最終実装との突き合わせ）

- 型・POST・GET の実際の名前と例は `agent-docs/progress/2026-10-19-wire-variant/contract.md` の verify-contract 節。実装は D1〜D5 と一致（`test-parallel.sh` 5846 passed、clippy/fmt 0）。
- 未完: task-core `CampaignIdentity.variant`・API 409 比較・子孫/retry 継承・実 org 適用・Assigned 検査は次段。**適用証明は未実施**で、本契約の完了とは別。
