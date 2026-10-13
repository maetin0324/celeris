---
task: 01M4M8D4AP4YY4F9TV09HG30SJ
wu: api-residue
status: done
---

# crates-residue / api-residue（task-api）

## 変更
- `crates/task-api/src/handlers/secrets.rs`: `put_secret` の注釈から `/providers/{id}` の参照を外し、secrets 自身の 404 規約を書く。
- `crates/task-api/tests/secrets_admin.rs`: 同じく試験注釈の `/providers/{id}` 参照を外す。
- `crates/task-api/tests/cluster_connect_admin.rs`: 注釈の `accounts/providers` の `reload`/`check` 参照を `POST /reload` に直す（試験の期待値 500 `internal` は変えない）。
- `crates/task-api/tests/routing.rs`: proxy log の `requested_model` を `celeris/cheap` から `qwen/qwen3.8-27b` に替えた。task-api の src はこの欄を読まないため、他の期待値（`log.model` = `claude-sonnet-5` は upstream_model 由来）は変わらない。

## 証拠
- `grep -rnE 'celeris/(frontier|standard|cheap)|/providers\b|accounts/providers' crates/task-api` → 残りは `tests/cos_operations.rs:103`（path traversal 試験 `/api/v1/tasks/../providers`。残す対象）の 1 件のみ。
- `cargo test -p task-api` → exit 0（全 test binary で 0 failed）。
- `cargo fmt -p task-api -- --check` → exit 0。
- 範囲: `git diff --stat "$CELERIS_WU_BASE" -- crates/` は `crates/task-api/` の 4 file のみ。

## 未解決事項
- なし（task-api の範囲）。他 crate の残りは core-residue・worker-fixtures・worker-route の担当。
