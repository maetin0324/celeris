---
title: アーキ v4 H R2 accounts — account の plan と作用を SCC の外の account_write.rs へ移す
tasks: [01M7T5KM3FW3430R5DT4QG5QKV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# accounts（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- [ADR 2026-11-19 cos-handlers-remaining-edges](../../adr/2026-11-19-cos-handlers-remaining-edges.md) の D2・D3 を実装した。
  account の plan と作用を `crates/task-api/src/handlers/accounts.rs` から新しい `crates/task-api/src/account_write.rs` へ移し、
  `Effect`・`effect_result` を `crates/task-api/src/cos/operations.rs` から `crates/task-api/src/problem.rs` へ移した（re-export は無い）。
- HTTP の route（`handlers/accounts.rs`）と CoS の `account.*`（`cos/ops/admin.rs`）は `account_write` を一方向に呼ぶ。
  `account_write` は `cos`・`handlers`・`state` を use せず、どの SCC にも入らない。
- task-api の SCC の組は 261 → 230（消えた 31・増えた 0）、cos → handlers の行は 50 → 29（`super::` 4 → 0）、
  handlers → cos の行は 19 → 14（before は `git merge-base HEAD main`、after は HEAD。http-tools の分を含む）。
- response・拒否理由・監査（`file_op`・`external`・`audit.reject` の呼び方と対象 id）・認可は変えていない。ADR と違えた点は
  ADR の末尾の付記に 8 項目で書いた。

## 1. 変更

commit（件名）:

- `arch-v4 H R2 accounts: account の plan と作用を account_write.rs へ、Effect を problem.rs へ移す`
- `arch-v4 H R2 accounts: architecture-map・ADR 付記・進捗 accounts.md を書く`
- `arch-v4 H R2 accounts: architecture-map の account_write の行の呼び出し元を相対 path にする`（前回の run の check で
  repo-health の `arch-map.wrong_path` が 2 件 fail した。全角括弧の中の backtick path は直前の path の dir からの相対として
  展開される（`scripts/dev/check-architecture-map.py` の `collect_backtick_paths`）ので、`crates/task-api/src/` を外した）

### 1.1 file ごとの差分（基準は `$CELERIS_WU_BASE`、`git diff --numstat`、crates/ だけ）

| file | + | − | 中身 |
|---|---|---|---|
| `account_write.rs`（新規） | 580 | 0 | 移した定義 506 行・`AccountEnv` 10 行・`limit_adapter` 25 行・module の doc と import と空行 39 行 |
| `handlers/accounts.rs` | 30 | 577 | 845 → 298 行。route だけを残し、`account_write` を呼ぶ |
| `cos/ops/admin.rs` | 43 | 61 | `account.*` の 3 つの腕を `account_write` 経由にした。`external` の型の path |
| `cos/operations.rs` | 1 | 10 | `Effect`・`effect_result` の定義を消した。import 1 行 |
| `problem.rs` | 23 | 0 | `Effect`・`effect_result`（移動）と `blocking`（`ApiState::blocking` の本体） |
| `state.rs` | 20 | 8 | `account_env`、`blocking` の委譲、`account_stats`・`stats` を `Arc<Mutex<…>>` に |
| `lib.rs` | 2 | 0 | `mod account_write` と doc |
| `chat/attachments.rs`・`model_catalog.rs`・`notify.rs`・`releases.rs`・`handlers/{reload,system}.rs` | 13 | 16 | `Effect`・`effect_result` の path |
| 計 | 712 | 672 | task-api の src は 40 行増えた（行の純減はこの WorkUnit の目的ではない） |

### 1.2 移した定義（base の `handlers/accounts.rs` の行 → `account_write.rs` の行。doc を含む）

| 定義 | base | 行数 | after | 違い |
|---|---|---|---|---|
| `const IMPLICIT_ADAPTER`・`MAX_RUNS_RANGE` | 24–27 | 4 | 4 | 無し |
| `pool_adapter` | 29–36 | 8 | 8 | 無し |
| `pool_source_id` | 38–44 | 7 | 7 | `pub(crate)` にした（HTTP の一覧が使う） |
| `is_implicit_source` | 46–51 | 6 | 6 | `&ApiState` → `&AccountEnv` |
| `resolve_pool_account` | 53–76 | 24 | 24 | 同上 |
| `effective_max_runs` | 78–84 | 7 | 7 | 無し |
| `account_items` | 86–217 | 132 | 130 | `ApiState::blocking` → `problem::blocking`、帳簿は `Arc` を写す。`pub(crate)` に |
| `implicit_view` | 219–246 | 28 | 28 | 無し |
| `parse_limit_patch` | 279–305 | 27 | 27 | 無し |
| `set_account_limit_effect` | 348–438 | 91 | 88 | `require_active` を外した（呼び出し元が先に呼ぶ） |
| `create_account_op` | 480–533 | 54 | 54 | `&AccountEnv` |
| `clear_stale_limit` | 535–566 | 32 | 31 | `problem::blocking` |
| `delete_account_effect` | 585–620 | 36 | 34 | `require_active` を外した |
| `account_admin_error` | 622–633 | 12 | 12 | `pub(crate)` にした（login の route が使う） |
| `check_account_effect` | 650–696 | 47 | 46 | `require_active` を外した。`checked_at` は同じ書式を中で作る（ADR 付記 6） |
| 計 | | 515 | 506 | |

`Effect`（doc 2 行 + 1 行）と `effect_result`（doc 1 行 + 3 行）の 7 行と空行は `cos/operations.rs` から `problem.rs` へ移した。

### 1.3 消した定義・まとめた処理（削除の分）

| 対象 | before | after | 差 |
|---|---|---|---|
| `account.limit_set` の adapter の解決（HTTP の `patch_account_limit` 25 行 + `patch_account_limit_inner` 14 行 + CoS の腕 51 行） | 90 | 73（HTTP 20 + CoS 28 + `limit_adapter` 25） | −17 |
| `patch_account_limit_inner` | 14 | 0 | `patch_account_limit` にまとめた（上の行に含む） |
| `ApiState::blocking` の本体 | 4 | 1 | 本体は `problem::blocking`（doc と where を含む 11 行）へ |

足した行: CoS の `account.delete`・`account.check` の腕は 29 → 36 行、HTTP の `delete_account`・`check_account` は各 +2 行
（`AccountEnv` の写しと `require_active` の呼び出し）、`AccountEnv` 10 行、`ApiState::account_env` 14 行。

### 1.4 高 risk の path（review profile の class）に当たる変更

| path | class | 変えたもの |
|---|---|---|
| `crates/task-api/src/cos/operations.rs` | authz | `Effect`・`effect_result` の定義（9 行）を消し、import を `crate::problem::{ApiProblem, Effect}` にした。`effect_value`・`OperationAudit`・`file_op`・`external` と principal の判定は変えていない |
| `crates/task-api/src/releases.rs` | selfdeploy | `crate::cos::operations::{effect_result, Effect}` → `crate::problem::…` の型の path 2 行だけ |

`crates/task-api/src/middleware.rs`（authz）は変えていない。CoS は `crate::middleware::require_active` を `external` の
closure の中で呼ぶ（今までは作用の中で呼んでいた。拒否は今と同じ `Ok(Err(standby))` で記録される）。

## 2. 不変条件と試験

| 不変条件 | 確かめた試験・検査 |
|---|---|
| account の response・拒否理由・監査が変わらない | `llm_accounts_admin` の全件、`cos_delivery_suite` の `cos_ops_admin_config_account_refusals_are_recorded_with_their_reason`（拒否 17 場面と監査の対象 id）・`cos_ops_admin_config_account_success_results_keep_their_shape`・`cos_ops_admin_config_daemon_and_external_effects_run_once` |
| `cos-audit-same-tx`・`api-principal-from-token` | `cos_operations`・`auth_and_guards`・`cos_delivery_suite` の `org_write_contract`・`tx_boundary_contract` |
| `account_write` と `Effect` の置き場所が SCC の外、組が増えない | measure.snippet の compare（exit 0） |
| 旧い経路が残らない | ADR §7 の `git grep` 3 本がどれも 0 行、WorkUnit の grep 検査が exit 0 |

試験は足していない・消していない・弱めていない（契約は WorkUnit `contract` が足した試験で固定済み）。

## 3. 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、measure の before は `git merge-base HEAD main`）

- `! git grep -nE 'handlers::accounts|super(::super)*::handlers' -- crates/task-api/src/cos && ! git grep -nE 'crate::(cos|handlers|state)\b' -- crates/task-api/src/handlers/accounts.rs crates/task-api/src/account_write.rs | grep -vE 'handlers/accounts.rs:.*crate::(handlers|state)' && ! git grep -nE 'type Effect<' -- crates/task-api/src/cos` → exit 0
- ADR §7 の旧い経路の `git grep` 3 本 → どれも 0 行
- `python3 -I agent-docs/progress/2026-11-19-arch-v4-h-r2/measure.snippet compare <before.json> <after.json> crates/task-api/src/http_io crates/task-api/src/account_write` → exit 0、`OK`。
  ta_scc_units 49 → 49、ta_scc_pairs 261 → 230（消えた 31・増えた 0）、ta_scc_lines 622 → 538、handlers_to_cos_lines 19 → 14、
  cos_to_handlers_lines 50 → 29（crate:: 46 → 29、super:: 4 → 0）。http-tools の後の HEAD（232 組）からこの WorkUnit で消えた組は
  notify → cos と releases → cos の 2 つ（`Effect` の移動）
- `cargo nextest run -p task-api --test llm_accounts_admin --test cos_delivery_suite --test cos_operations --test auth_and_guards` → exit 0（74 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6132 passed・21 skipped、web の tsc -b と unit 705 passed、repo_health_gate all checks passed、
  repo-health verdict=pass new=0 worsened=0、`arch-map.wrong_path` findings=0、`deps.cycle` pass）
- `python3 -I scripts/dev/check-architecture-map.py --root .` → exit 0（1103 件の path を確認）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 4. 未解決

- WorkUnit `contract` が見つけた所見（contract.md §4）は直していない。HTTP の `create_account` は `clear_stale_limit` に
  `view.source_id`（`claude_oauth`）を渡し、CoS は adapter 名（`claude-code`）を渡す。上限の保存値は adapter 名の key で読み書き
  されるので、HTTP で作り直した account は古い上限を引き継ぐ疑いがある。この WorkUnit は移動だけで挙動を変えない約束なので、
  渡す値はそのまま移した（`handlers/accounts.rs` の `create_account`）。
- HTTP と CoS で値が違う 4 場面（contract.md §3）も変えていない。

## 5. 提案

- 上の `clear_stale_limit` の key を adapter 名に揃える修正と、HTTP の作成で古い上限が消えることを確かめる試験を別の task で行う
  （応答・拒否理由・認可は変わらない）。
