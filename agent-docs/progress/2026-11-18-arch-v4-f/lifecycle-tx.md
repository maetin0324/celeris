---
title: アーキ v4 F lifecycle-tx — task-ops の案件の操作の closure の transaction を消し、HTTP adapter が write_tx の中で human epoch を加算する
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# lifecycle-tx（WorkUnit）

ADR [2026-11-18-knowledge-lifecycle-tx-boundary](../../adr/2026-11-18-knowledge-lifecycle-tx-boundary.md) の D4 と D7（project-controller の行）を実装した。
基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。検査は提出時の HEAD（commit の直前の作業ツリー、Rust と文書は commit と同じ内容）で実行した。

## 変更（commit の件名）
- `arch-v4 F lifecycle-tx: 案件の操作の closure の transaction を消し、HTTP は write_tx の中で HumanEpoch::bump_tx を呼ぶ`

## 消したもの
| 場所 | 消したもの |
|---|---|
| `crates/task-ops/src/lifecycle.rs` | `apply_project_action`（doc comment と本体。`also: F`、`F: FnOnce(&rusqlite::Connection) -> Result<(), StoreError>` の closure 引数と、その中で開いていた `store.write_tx`） |
| `crates/task-api/src/lifecycle.rs` | `project_action_op`（`Option<&OperationAudit>` を受けて `Applied` を返す共有の操作）と `project_action_name`（CoS の腕は同じ文字列の `matched.action` を使う） |
| `crates/task-api/src/cos/ops/projects.rs` | `project.*` の腕の `audited(..)` の包みと `HumanEpoch::NotCounted` の受け渡し（腕は `HumanEpoch` を参照しない。出現数 1 → 0） |

## 公開の signature の前後
| 関数 | before | after |
|---|---|---|
| `task_ops::lifecycle::apply_project_action` | `pub fn apply_project_action<F>(store: &SqliteStore, id: ProjectId, action: ProjectAction, now: OffsetDateTime, also: F) -> Result<ProjectLifecycle, OpsError> where F: FnOnce(&rusqlite::Connection) -> Result<(), StoreError>` | 削除 |
| `task_ops::lifecycle::project_action_result` | 無し | `pub fn project_action_result(store: &SqliteStore, id: ProjectId, change: ProjectChange, cancelled: Vec<TaskId>) -> Result<ProjectLifecycle, OpsError>`（旧 `apply_project_action` の後半。書いた後に読むだけ） |
| `task_ops::lifecycle::apply_project_change_tx` | `pub fn apply_project_change_tx(tx: &rusqlite::Connection, id, change: &ProjectChange) -> Result<Vec<TaskId>, StoreError>` | `tx: &rusqlite::Transaction<'_>`（transaction の外の接続で呼べない）。他は同じ |
| `task_ops::lifecycle::plan_project_action` | 変更なし | 変更なし |
| task-api `lifecycle::project_action_op`・`project_action_name` | `pub(crate)` | 削除 |
| task-api `lifecycle::project_trigger` | private | `pub(crate)`（HTTP と CoS の腕が使う） |

数: task-ops の `lifecycle.rs` の `pub fn` は 7 → 7（1 削除・1 追加。増やさない）。task-api の `lifecycle.rs` の `pub(crate) fn` は 4 → 3。

## 新しい経路
- HTTP（`task-api/src/lifecycle.rs` の `project_action`）: `state.blocking` の中で `plan_project_action`（失敗は `ops_problem(store, e, Some(trigger))`）→
  `ProjectChange::Unchanged` でなければ `store.write_tx::<_, ApiProblem, _>(|tx| { apply_project_change_tx(tx, ..)?; human.bump_tx(tx, Some(project_id))?; .. })`
  → `project_action_result`。`write_tx` の失敗は `From<StoreError> for ApiProblem`（`store_problem`）で、旧経路の `OpsError::Store` → `store_problem` と同じ写像。
  加算の規則・`now_utc`・`COALESCE_SECS = 30` は `human_epoch.rs` のまま。Unchanged と拒否では書かず数えない。
- CoS（`cos/ops/projects.rs` の `project.*` の腕）: `plan_project_action` の失敗を `ops_problem` → `audit.reject(store, "project", &id.to_string(), ..)`、
  成功なら `audit.apply(store, "project", &id.to_string(), matched.action, |tx| apply_project_change_tx の後に {"project_id", "cancelled_tasks"})`。
  監査の行の target・action 名・result の JSON は旧経路と同じ。Unchanged でも operation の行を書くのは同じ。
- task-ops は transaction を開かず、human epoch を加算しない（`git grep -n project_controller_human_bump_tx -- crates/task-ops/src` は 0 件）。
- `task_pause_op` は変えていない。新しい trait・Port・flag は足していない。

## 文書
- `agent-docs/adr/2026-11-04-project-controller.md`: 付記 A1 の 7 の表の `POST /projects/{id}/…` の行を新しい経路（handler の `write_tx` の 1 tx で
  `apply_project_change_tx` → `HumanEpoch::bump_tx`）に書き換えた。9 の表（`human_epoch.rs` の module doc が指す表）には案件の操作の行が無かったので、
  HTTP（数える）と CoS の `project.*`（数えない）の 2 行を足した。末尾に「付記 F」を足し、本文から F の ADR へリンクした。
- `crates/task-api/src/human_epoch.rs` の module doc: 「`/cos/operations` の監査経路（`*_op` の `audit = Some`）」が `project.*` の腕（`*_op` を通らない）と
  食い違うので、「`OperationAudit` の transaction。`*_op` の `audit = Some` と、`project.*` のように腕が `reject`・`apply` を直接呼ぶもの」に直した。
- task-ops の `lifecycle.rs` の `ProjectChange` の doc comment、task-api の `lifecycle.rs` の module doc を新しい経路に合わせた。
- `docs/architecture-map.md` は owner の file が変わらないので変えていない。

## 行数（`git diff --numstat $CELERIS_WU_BASE`）
| 区分 | 追加 | 削除 | 純 |
|---|---|---|---|
| production（task-ops `lifecycle.rs` 12/25、task-api `lifecycle.rs` 22/54、`cos/ops/projects.rs` 15/8、`human_epoch.rs` 2/1） | 51 | 88 | −37 |
| 試験（task-ops `lifecycle/tests.rs` 4/1、dispatcher `tests/project_wake.rs` 8/10） | 12 | 11 | +1 |
| ADR（project-controller） | 21 | 1 | +20 |

`crates/task-dispatch/src/dispatcher/` の全 `.rs` の行は 135,764 → 135,762（−2。試験の呼び出しの書き換えだけ）。
3 file の `rusqlite` の行（`grep -c rusqlite`）: `knowledge.rs` 7 → 7（この unit の範囲外）、task-ops の `lifecycle.rs` 2 → 1、task-api の `lifecycle.rs` 0 → 0。

## 試験の書き換え（assert は変えていない）
- `crates/task-ops/src/lifecycle/tests.rs` の `act`: `plan_project_action` → `store.write_tx(apply_project_change_tx)` → `project_action_result`。
  旧 `act` は `also` に `|_| Ok(())` を渡していたので、加算の無い同じ書き込み。Unchanged でも空の `write_tx` を開くだけで、何も書かない。
- `crates/task-dispatch/src/dispatcher/tests/project_wake.rs` の `project_wake_human_races_pause`: `plan_project_action` → `write_tx` の中で
  `apply_project_change_tx` の後に `SqliteStore::project_controller_human_bump_tx(tx, &id.to_string(), now, 0)`（coalesce 0 を明示したまま）。

## 証拠（提出時の HEAD の作業ツリーで実行）
| 受け入れ条件 | command | 結果 |
|---|---|---|
| 0 | `git grep -nE 'apply_project_action\|FnOnce\(&rusqlite::Connection\)\|Fn\(&rusqlite::Connection\)' -- crates/task-ops/src/lifecycle.rs crates/task-ops/src/lifecycle`、`grep -n 'pub fn apply_project_change_tx' -A3 crates/task-ops/src/lifecycle.rs` | 0 件（exit 1）、`tx: &rusqlite::Transaction<'_>` |
| 1 | `git grep -n project_controller_human_bump_tx -- crates/task-ops/src` | 0 件（exit 1）。HTTP の加算は `human.bump_tx` の 1 か所 |
| 2 | `git grep -n project_action_op -- crates/task-api/src crates/task-ops/src` | 0 件（exit 1） |
| 3・5 | `cargo nextest run -p task-ops -p task-api -p task-dispatch -E 'test(/lifecycle/) \| test(/tx_boundary_contract/) \| test(/project_wake_api_human/) \| test(/project_wake_human_races/) \| test(/project_controller/) \| test(/cos_ops_projects_cron/)'` | exit 0、76 passed（task-api 45・task-dispatch 21・task-ops 10。`tx_boundary_contract::` 8 件、`project_wake_api_human_*` 24 件、`project_wake_human_races_pause` を含む） |
| 5 | `bash scripts/dev/test-affected.sh`（対象 celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker と web） | exit 0。nextest 6866 passed（22 skipped）、web の unit 705 passed（101 file）、repo_health_gate all checks passed。real 14m40s |
| 5 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 5 | `cargo fmt --all -- --check` | exit 0 |
| 5 | `python3 scripts/dev/repo-health/repo_health.py --repo "$(pwd)" --base "$CELERIS_WU_BASE" --head worktree --policy-version 1 --out $TMPDIR/health.json` | exit 0、`deps.cycle` pass、`size.responsibility-growth` pass |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` | 上の 7 file とこの進捗 file だけ |

`tx_boundary_contract.rs` と `routing_budget_suite/project_wake_human.rs` は変えていない（`git diff --stat` に無い）。

## 未解決
- 無し。knowledge.rs（D1〜D3）は並行の WorkUnit `provenance-read`・`knowledge-pin` の範囲で、この unit は触っていない。

## 提案
- 無し。
