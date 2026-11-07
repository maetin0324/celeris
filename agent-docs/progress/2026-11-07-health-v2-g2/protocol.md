---
title: 健全性v2 G2 protocol — worker protocol の health 文脈と構造化 health verdict
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 protocol — worker protocol の health 文脈と構造化 health verdict

完了日: 2026-11-07

WorkUnit `protocol`。基準は `$CELERIS_WU_BASE`。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の D2.4・D3.4・D4.1・D5.1。

## 変更
件名「G2 protocol: health 文脈と構造化 health verdict の wire 型」の commit。
- `crates/task-core/src/design_review.rs`（新規）: `ReviewPolicy { owner_map, high_risk: Vec<PathClass>, invariants, definition_sha256 }`・`PathClass { class, paths }`（D2.4）。`assess_risk` は入れていない（D3.3 は後続の dispatch unit）。
- `crates/task-worker/src/protocol/health.rs`（新規）: `HealthReviewInput`・`HealthRepoInput`・`DiffStat`・`PathStat`・`OwnerRow`・`HealthFindingsSummary`・`HealthFindingCounts`・`HealthReportFinding`・`HealthPolicyState`（D4.1）、`HealthVerdictOut`・`HealthFindingOut`・`HealthFindingKind`（固定 7 種、snake_case）・`HealthSeverity`（blocking | advisory）・`PolicyVerdictOut`（D5.1）。`HealthFindingKind::as_str`・`allows_local_repair`（`removal_missing`・`test_change_unjustified` だけ true）を後続の review_verdict の写像用に置いた。
- `protocol.rs`: `ReviewRequest.health: Option<HealthReviewInput>`、`ReviewOutput.health: Option<HealthVerdictOut>`、`ExecutionPlannerContext.review_policy: Option<ReviewPolicy>`。どれも `serde(default, skip_serializing_if = "Option::is_none")` で、旧い JSON は読め、`None` のときの出力は従来と同じ。`PROTOCOL_VERSION` は 4 のまま。
- 構築箇所: `task-dispatch` の `planner_flow.rs`（`review_policy: None`。同じ literal の `tree` を `then(..).transpose()?` に縮めた）と `review.rs`（`ReviewRequest` の組み立てを `ReviewHumanContext::into_request` に移し `health: None`）。値を入れる配線は dispatch unit。
- `docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1` で再生成、`worker-protocol.md` の context 表と §10.2 に新しい欄と 7 種を書いた。

## ADR との違い（判断）
- `review_policy` は `RunContext` の直下でなく `ExecutionPlannerContext`（ADR D3.4 の「planner 欄」）に置いた。`RunContext` の直下に足すと欄の数が 41 になり、repo health の `size.responsibility-growth`（閾値 40）が fail になった。reviewer には同じ base 側の不変条件が `HealthRepoInput.invariants` で届く。execute run に方針を渡す必要が出たら、prompts・dispatch unit が判断する。
- `HealthReviewInput.brief`（D4.1）、`RunContext.design_review`・`DesignReviewRequest`・`ReviewOutput.design`・`DesignVerdictOut`（D3.5、U3）は入れていない。`DesignBrief` は並行の plan-brief unit が `task_core::execution_plan` に作るので、型ができた後の dispatch unit で足す。
- `HealthReportFinding.severity`・`delta`、`HealthFindingsSummary.verdict`、`HealthPolicyState.source` は文字列にした。値の正本は repo health の report の schema（`report-v1.schema.json`）で、Rust 側に同じ enum を二重に持たない。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-worker -E 'test(/health_protocol_|committed_schema_matches_generated/)'` → 7 passed（health_protocol_ 6 件: request の往復・verdict の往復・planner の review_policy の往復・省略時の既定と非出力・未知の kind/severity の拒否・7 種と schema の一致）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 7848 passed / 23 skipped、test-ts-verify 20 passed、repo-health verdict=pass fail_grade=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 上の「ADR との違い」の 2 点（`brief`・design review の型、execute run への方針の配布）は dispatch・prompts unit で扱う。
- `task-dispatch` の dispatcher module（35,337 行、閾値 8,000）と `run_reviewer_inner`・`run_worker` は既に閾値を超えており、行を足すと `size.responsibility-growth` の worsened で test-affected が落ちる。dispatch unit は配線で行が増える分を同じ module の中で減らすか、新しい module（ADR の `review_health.rs`）に置く必要がある。

## 提案
- なし。
