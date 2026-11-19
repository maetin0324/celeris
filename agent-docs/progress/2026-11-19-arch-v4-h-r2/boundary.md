---
title: アーキ v4 H R2 boundary — 高 risk の 5 file の変更を ADR 付記に書き、不変条件の試験を走らせる
tasks: [01M7T5KM3FW3430R5DT4QG5QKV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# boundary（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- task の差分が変えた高 risk の class の 5 file（authz: `crates/task-api/src/cos/operations.rs`・`crates/task-api/src/handlers/authority.rs`、
  credential: `crates/task-api/src/handlers/secrets.rs`、selfdeploy: `crates/task-api/src/releases.rs`、transaction: `crates/task-api/src/knowledge.rs`）の
  class・不変条件・変更・試験を [ADR の付記](../../adr/2026-11-19-cos-handlers-remaining-edges.md)（「付記（2026-11-19、WorkUnit `boundary`）」）に表で書いた。
- 5 file の `git merge-base HEAD main` からの差分は import と型・関数の path の書き換えだけだった（差分の形の検査の出力が空、exit 0）。
- 不変条件の試験は 222 件すべて通った（exit 0）。`crates/` は変えていない。repair の申告は無い。

## 1. 経緯（計画 v2 の理由）

v1 の計画は write path を dir（`crates/task-api/`）だけで書いた。計画の設計 review は高 risk の class の path に当たらず、final review の
health の `diff_design` が「設計 review が覆わない境界の変更（reviewer run が無く判定できない）」として 5 file と 6 つの不変条件を挙げて落ちた。
v2 の計画はこの WorkUnit の `context.paths` に 5 file を file 単位で書き、計画の設計 review に覆わせる。

## 2. 5 file の差分（`git merge-base HEAD main` → HEAD）

`git diff --numstat` と、変えた commit（件名）:

| file | +/− | commit（件名） |
|---|---|---|
| `crates/task-api/src/cos/operations.rs` | +4 −13 | `arch-v4 H R2 http-tools: HTTP の道具を handlers.rs から http_io.rs へ移し、cluster_ids を ApiState の method にする`、`arch-v4 H R2 accounts: account の plan と作用を account_write.rs へ、Effect を problem.rs へ移す` |
| `crates/task-api/src/handlers/authority.rs` | +3 −0 | 同上（http-tools） |
| `crates/task-api/src/handlers/secrets.rs` | +2 −1 | 同上（http-tools） |
| `crates/task-api/src/releases.rs` | +9 −9 | 同上（http-tools・accounts） |
| `crates/task-api/src/knowledge.rs` | +1 −1 | 同上（http-tools） |

行ごとの中身は ADR の付記の表にある。`cos/operations.rs` の −13 のうち 9 行は `Effect` 型と `effect_result` の定義（doc を含む）で、
`crate::problem` へ移した。`OperationAudit` の transaction・CoS の principal の判定・`releases.rs` の `require_admin` の行は差分に無い。

## 3. 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、WorkUnit の範囲は `$CELERIS_WU_BASE`）

- 差分の形の検査（ADR の付記の 2 本の `git diff -U0 … | grep`。WorkUnit の check と同じ）→ 出力 0 行、exit 0。
- `cargo nextest run -p task-api --lib --test run_worker_suite --test authority_ledger --test knowledge --test cos_operations --test auth_and_guards --test cos_delivery_suite --test llm_accounts_admin`
  → exit 0。`Starting 222 tests across 8 binaries (2 tests skipped)`、`Summary [6.345s] 222 tests run: 222 passed, 2 skipped`。
  skipped の 2 件は lib の `#[ignore]` の計測（`stats::execution_metrics_comparison_tests::indexed_summary_timing_2000_tasks_20_events`・
  `timeline::tests::profile_timeline_against_a_db_copy`）で、この変更とは関係しない。

- `sh scripts/dev/check-doc-links.sh`（ok）・`sh scripts/dev/check-adr-numbers.sh`（ok、463 files）・`sh scripts/dev/progress-index.sh --check`（ok）→ いずれも exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（WorkUnit の範囲は docs だけなので「試験対象の変更なし」、repo_health_gate: all checks passed）。
  `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲の検査（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は ADR・進捗・この file の 3 path だけ。`crates/` の変更は 0。

binary ごとの件数（`cargo nextest list` の同じ引数で数えた。ignored を除く）:

| binary | 件数 | うち不変条件に当てた module |
|---|---:|---|
| lib（`task_api`） | 101 | `releases::tests` 2・`secrets::tests` 7・`knowledge::` 1・`cos::operations::` 1・`principal::tests` 3・`middleware::tests` 3 |
| `run_worker_suite` | 15 | `secrets_admin` 12 |
| `authority_ledger` | 22 | 全件 |
| `knowledge` | 10 | 全件 |
| `cos_operations` | 8 | 全件 |
| `auth_and_guards` | 11 | 全件 |
| `cos_delivery_suite` | 36 | `cos_ops_admin_config` 6・`org_write_contract` 6・`tx_boundary_contract` 8 |
| `llm_accounts_admin` | 19 | 全件 |
| 計 | 222 | |

不変条件と試験の対応:

| 不変条件 | 試験 | passed |
|---|---|---:|
| `credential-stays-in-broker` | `run_worker_suite` の `secrets_admin` 12、lib の `secrets::tests` 7 | 19 |
| `unpromoted-release-no-takeover` | lib の `releases::tests` 2、`cos_delivery_suite` の `cos_ops_admin_config` 6（promote を 1 回だけ走らせる） | 8 |
| `api-principal-from-token` | `auth_and_guards` 11、`authority_ledger` 22、`llm_accounts_admin` 19 | 52 |
| `cos-audit-same-tx` | `cos_operations` 8、`cos_delivery_suite` の `org_write_contract` 6・`tx_boundary_contract` 8 | 22 |
| `task-state-in-sqlite`・`lifecycle-human-epoch-same-tx` | `knowledge` 10、`cos_delivery_suite` の `tx_boundary_contract` 8（上と同じ 8 件） | 18 |

## 4. 未解決

- 無し。試験も差分の形の検査も通ったので、`crates/` の修正（repair）は要らない。
