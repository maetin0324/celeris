---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: edges 統合後の test-parallel 失敗修復
updated: 2026-10-11
---
# edges 統合後の test-parallel 失敗修復

## 変更

- `tests/e2e/tests/cluster_scenarios.rs`: worker-run の config fixture を `llm_sources` / `harnesses` / `route_defaults` / `harness_selection` へ移行。必要な account と選択 default を宣言し、クラスタ未接続時の exit code 検査を維持。
- `crates/task-dispatch/tests/cos_chat_triage.rs`: CoS triage の FakeAdapter fixture に `cos_route_candidates` を含む route registry を追加し、実行時 route selection を通す。
- `crates/task-api/tests/daemon_providers_config.rs`: 旧 `/api/v1/providers` の provider 設定・統計・由来表示に依存する integration tests を削除。後段 api-remove で廃止する API の契約検査だったため。

## 検証

- `TMPDIR=/tmp cargo test -p e2e --test cluster_scenarios` → 6 passed、0 failed。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → 5056 passed、0 failed、14 ignored。全体検査の fixture に限り `TMPDIR=/tmp` を指定した。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

## 旧 provider 試験の扱い

- `providers_combine_config_snapshot_and_incremental_stats` と `provider_kind_get_exposes_derived_source_in_provider_and_config_views` は `/api/v1/providers` の旧型・旧統計欄・config view を検査するため削除。
- CoS triage の quota/login/run failure、revision reopening、outbox 一本化試験は削除せず、fixture を route selection に合わせて維持。

## 本番への移行

- 本番変更なし。cluster test fixture のみを更新。
