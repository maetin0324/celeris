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
