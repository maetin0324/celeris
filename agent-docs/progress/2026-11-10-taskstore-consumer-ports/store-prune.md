---
title: 健全性v2 R3 store-prune — 未参照の TaskStore method と B0 割り当ての未参照 store 関数の削除
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R3 store-prune

基準は `git merge-base HEAD main`（WU の base は件名「integrate wu/ports-design (phase design)」）。対象は [prune-list.json](prune-list.json) の `fns` 17 名。

## 着手時の呼び手の確認
- `git grep -nw <name> -- crates` と `git grep -nw <name>_impl -- crates` を 13 名（TaskStore method 4・B0 割り当て 9）について実行した。
- TaskStore method 4 個は、宣言（`task_store.rs`）・転送（`task_store_impl.rs`）・本体（`<name>_impl`）の外に出なかった。B0 の 9 個は定義の 1 行だけだった。
- 呼び手が見つかって list から外した名前は無い。`prune-list.json` は変えていない。

## 消したもの
| 名前 | 種類 | 消した場所 | 理由 |
|---|---|---|---|
| `milestone_transition_status` | TaskStore method | `task_store.rs` の宣言・`task_store_impl.rs` の転送 | 呼び手が無い |
| `milestone_transition_status_impl` | 本体 | `store/projects.rs` | 上の転送だけが呼んでいた |
| `project_plan_decide_apply` | TaskStore method | 宣言・転送 | 呼び手が無い。案件計画の Go/No-Go は ADR-0079 で廃止済み |
| `project_plan_decide_apply_impl` | 本体 | `store/approvals.rs` | 上の転送だけが呼んでいた |
| `project_plan_apply` | TaskStore method | 宣言・転送 | 同上 |
| `project_plan_apply_impl` | 本体 | `store/approvals.rs` | 上の転送だけが呼んでいた |
| `project_set_text` | TaskStore method | 宣言・転送 | 呼び手が無い。CoS の監査付き操作と API は `SqliteStore::project_set_text_tx` を使い、こちらは残した（doc comment を移した） |
| `project_set_text_impl` | 本体 | `store/projects.rs` | 上の転送だけが呼んでいた |
| `authority_audit_record_tx` | B0 `unref-authority-audit-record-tx` | `store/authority.rs` | 定義だけ。同じ行を書く `authority_audit_record`（`write_tx` で `audit_insert`）は残した |
| `chat_legacy_reply` | B0 `unref-chat-legacy-reply` | `store/legacy.rs` | 定義だけ |
| `insert_route_judgment` | B0 `unref-insert-route-judgment` | `store/route_records.rs` | 定義だけ |
| `roadmap_last_status_tx` | B0 `unref-roadmap-last-status-tx` | `store/roadmap.rs` | 定義だけ |
| `route_decision_get` | B0 `unref-route-decision-get` | `store/route_records.rs` | 定義だけ |
| `route_judgment_get` | B0 `unref-route-judgment-get` | `store/route_records.rs` | 定義だけ |
| `route_judgments_for_hashes` | B0 `unref-route-judgments-for-hashes` | `store/route_records.rs` | 定義だけ |
| `route_outcome_get` | B0 `unref-route-outcome-get` | `store/route_records.rs` | 定義だけ |
| `task_unit_leaf_shortfalls` | B0 `unref-task-unit-leaf-shortfalls` | `tree/gate.rs` | 定義だけ |

付随して消したもの（上の削除で使い手が無くなった型。旧い仕組みを残さない）:
- `ProjectPlanApply`・`ProjectPlanMilestoneChange`（`store/mod.rs` の定義と `lib.rs`・`task_store.rs`・`task_store_impl.rs` の import）。`project_plan_apply` の入力だけに使われていた。
- `StoredRouteJudgment`（`store/route_records.rs` の定義と `store/mod.rs` の re-export）。消した 3 関数（`insert_route_judgment`・`route_judgment_get`・`route_judgments_for_hashes`）だけが使っていた。
- `store/mod.rs` の module map の `approvals.rs` の行から案件計画の Go/No-Go の記述を消した。`store/mod.rs` の使われなくなった import（`MilestoneId`・`MilestoneStatus`・`Trigger`）を消し、それを `use super::*` で借りていた `store/tests.rs` に同じ 3 つの import を足した（試験の中身は変えていない）。

## 試験
- 消した試験は無い。17 名を呼ぶ試験は crates/ に無かった（上の grep）。契約を守る試験が無かったので、規約 10 の「契約の廃止」を書く対象の試験も無い。
- 試験を足していない。削除だけで、守る不変条件は変えていない。

## 前後の計測（変更前 = 件名「integrate wu/ports-design (phase design)」、変更後 = 提出時の HEAD）
| 指標 | 前 | 後 | 差 | 数え方 |
|---|---|---|---|---|
| `task_store.rs` の fn 宣言 | 193 | 189 | −4 | `grep -c '^    fn ' crates/task-core/src/store/task_store.rs` |
| `dyn TaskStore` から見える method（supertrait 込み） | 308 | 304 | −4 | `measure.py --check-single-decl` の `fns` |
| store module の本体の行（size rule の `module_lines`） | 19676 | 19396 | −280 | `repo_health.py --base <前> --head worktree --policy-version 1 --rules size.responsibility-growth` の finding の `baseline.value`・`head_value`（単位 `crates/task-core/src/store`） |
| `SqliteStore` の method（size rule の `type_methods`、store module） | 729 | 713 | −16 | 同上（単位 `crates/task-core/src/store::SqliteStore`） |
| store module の物理行（`crates/task-core/src/store/` の .rs 全部） | 40947 | 40618 | −329 | `git ls-tree -r --name-only <rev> crates/task-core/src/store` の各 file を `git show` して `wc -l` |
| 差分全体（crates/） | | | +21 / −383 | `git diff --numstat` |

## 証拠コマンドと結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --check-pruned agent-docs/progress/2026-11-10-taskstore-consumer-ports/prune-list.json` → exit 0（`names=17 remaining=0`）
- `… measure.py --check-single-decl` → exit 0（`traits=17 fns=304 duplicates=0`）
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R3 …/slices.json` → exit 0（`write-set R3: ok`）
- `cargo check --workspace --all-targets` → 成功（警告は変更前からある `crates/celeris/tests/script_exhaustive.rs` の unused import 1 件と vendor の deno_core 1 件だけ）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh` → exit 0（`verdict=pass fail_grade=0 new=0 worsened=1`。worsened の 1 件は warning 級の `dup.state`（表名 `t`）で、ports-design が足した `measure.py` の docstring の例示が数えられたもの。この unit の変更ではない）
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 8179 件合格・23 skip、web typecheck・unit（104 file・712 件）合格、ts-verify 20 合格（Run #2、load average 20〜32）。
- 1 回目（Run #1、load average 80）は exit 100 で、task-worker の browser login の試験 3 件（`launcher_credential_post_login_consent_page_is_held_with_its_reason`・`launcher_credential_login_navigates_injects_submits_and_consumes_the_granted_lease`・`daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason`）が落ちた。3 件を `cargo nextest run -p task-worker -E 'test(=…) | …' --test-threads 1` で単独に再実行すると exit 0。task-worker は消した 17 名を使っていない（grep で 0 件）。

## 変更（commit の件名）
- `store-prune: 呼び手の無い TaskStore method 4 個と B0 割り当ての未参照 store 関数 9 個を消す`
- `store-prune: 進捗に削除の一覧と前後の計測を書く`
- `store-prune: 進捗に test-affected の再実行の結果を書く`

## 未解決
- `route_judgments` 表は migration に残り、crates/ に読み手も書き手も無くなった。migration と表には触れない範囲なので残した。
- `held_test_only` の 14 method は対象外のまま（ports-design の判断どおり）。

## 提案
- `route_judgments` 表を消すかは、行が 0 件であることを本番の DB で確かめてから、migration を足す別の task で決める（データを失わない規則）。
