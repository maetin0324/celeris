---
title: "preflight の ProjectDetail 応答契約を実 API に合わせる（P0-1）"
tasks: [01M54HYBTJK5YZ4P7GJSYQ8N3P]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# preflight の ProjectDetail 契約（P0-1）

有料実行・本測定・本番 host 変更なし。外部ネットワーク・LLM なし。

## 原因と修正
- 実 `GET /api/v1/projects/{id}` は `ProjectDetail`（`{"project":{"auto_integrate":..},"repos":[..],..}`）。旧 `check_project_rules` は
  トップレベルの `auto_integrate` を読み、常に unknown=true 扱いで正しい案件も拒否していた（平坦な fake JSON が見逃していた）。
- `crates/campaign/src/preflight.rs::check_project_rules` を `detail["project"]["auto_integrate"].as_bool().unwrap_or(true)` に直した。
  欠落・bool でない・project が無い・旧い平坦な形は従来どおり拒否（安全側は緩めていない）。
- repos: 実 `GET /projects/{id}/repos` は `RepoList {items:[ProjectRepo]}`（`name`/`is_primary`/`location.kind`）で、`repos["items"]` 前提は
  実 API と一致。変更不要（契約試験で実本文により確認）。`RealHost` は本文を加工せず渡すので変更なし。
- 旧形式（平坦 JSON）の試験を実形式へ直した（`tests/preflight.rs`）。共有の setup は `tests/preflight_setup/mod.rs` へ移した。

## 検証区分
- 実装済み: 上記修正。
- 実経路検証済み（実 task-api router・正式 DTO・一時 DB の store。HTTP は oneshot）: `tests/preflight_contract.rs`
  - `real_project_detail_with_auto_integrate_false_passes`（POST /projects と repos 追加で作った案件の本文が通る）
  - `real_project_detail_with_auto_integrate_true_is_rejected`（PATCH で true にした実本文）
  - `missing_or_malformed_auto_integrate_is_rejected`（欠落・"false"・0・null・object・project 欠落・平坦旧形式・null 本文）
  - `preflight_over_real_router_bodies_gates_external_execution`（`check` / `run_gated`: false は ok、拒否時は transport 呼び出し 0 件・ledger/out 作成なし）
- mock 検証: Host は実 router の本文を返す fake（HTTP 層は通らない。`RealHost` の ureq 経路自体は既存 CLI 試験の一時 HTTP server）。
- 未検証: 本番 task-api に対する実 GET（本番 host は触らない）。

## 証拠コマンド
- `cargo test -p celeris-campaign --test preflight --test preflight_contract`: preflight 11 passed / contract 4 passed
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `bash scripts/dev/test-parallel.sh`: exit 0（nextest 5621 passed / 0 failed / 15 ignored、doctest ok）

## 未解決事項
なし。dev-dependency に task-api / task-ops / axum / tower / tokio / time / rusqlite 等を足した（task-api の試験共有部品 `tests/common` を `#[path]` で再利用するため）。

## 提案
なし。
