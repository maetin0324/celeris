---
title: アーキ v4 H R8d rc-gate — R1〜R7 の後の H の branch で判定経路・prompt・反例の試験と本物の検査器の probe を再実行する
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# rc-gate（WorkUnit、replan の R8d）

## 0. 結論

- 17 行すべて pass だった（fail 0）。各行は [gate.tsv](gate.tsv)（列 `key result note`）。
  result は「期待どおりの結果だったか」で、検査器の verdict ではない（危険な probe は検査器が fail を出せば pass）。
- C の 6 ケース（`boundary_case_1_`〜`6_`）は、R1〜R7 が入った後の HEAD でも 6 件とも通った。
- prompt の試験は 10 件とも通った。内訳は `prompt_boundary_` 3 件、R1 が足した `prompt_full_objective_` 4 件と `planner_prompt_acceptance_` 3 件。
  前回の [gate.md](../gate.md) §2 の注意（3,000 文字の objective の末尾を確かめていたのは worker の prompt だけ）は、R1 の 7 件で planner・設計 review・final review の prompt にも広がった。
- 本物の検査器に当てた probe は前回と同じ結果だった。
  - probe a: task-api の SCC の中に、まだ無い向きの組 `lifecycle -> knowledge` を 1 行足した。`deps.cycle` が fail（new 1 件）になり、検査器は exit 1 を返した。
  - probe b: 同じ file に comment を 1 行足した。`deps.*` の new・worsened は 0 件で、検査器は exit 0 を返した。
  - probe なしの run も exit 0・new 0 だった。R1〜R7 の差分は、merge-base に対して new・worsened の finding を出さない。
- D・E・F の反例試験は 81 件とも通った（org_write_contract 6・tx_boundary_contract 8・task-dispatch の filter 67）。件数は前回の safety.tsv の arch_v4_counterexamples と同じ。
- `cargo clippy --workspace -- -D warnings` と `cargo fmt --all -- --check` は exit 0。
- probe の変更は残っていない（§4）。crates/・scripts/・web/ と policy の file は変えていない。全体の試験は走らせていない。

## 1. 対象と条件

- 対象の tree: この WorkUnit の HEAD（件名 `integrate wu/r6-release (phase fix2)`）。R1〜R7（R7 は R7a・R7b）の統合が入っている。
- health の base: `git merge-base HEAD main`（件名 `integrate wu/close (phase close)` の commit。アーキ v4 G までが入った main）。前回の gate.md と同じ base。
- `git diff --quiet "$(git merge-base HEAD main)" HEAD -- scripts/dev/repo-health` は 0。検査器は base のものと同じ内容。
- host は home-dev、時刻は 2026-11-20T10:04Z〜10:30Z、開始時の 1 分 load は 2.14。
- nextest の summary の `skipped` は filter から外れた試験の数で、実行時の skip ではない。

## 2. 試験の再実行（この unit の checks）

| key | コマンド | exit | 件数 | 時間 |
|---|---|---|---|---|
| case1〜case6・prompt_boundary・prompt_full_objective・planner_prompt_acceptance | `cargo nextest run -p task-dispatch -p task-worker -E 'test(/boundary_case_\|prompt_boundary_\|prompt_full_objective_\|planner_prompt_acceptance_/)'` | 0 | 16 tests run: 16 passed, 3224 skipped | build 込み 114 秒、試験 11.0 秒 |
| org_write_contract | `cargo nextest run -p task-api --test cos_delivery_suite org_write_contract` | 0 | 6 tests run: 6 passed, 44 skipped | build 込み 63 秒 |
| tx_boundary_contract | `cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract` | 0 | 8 tests run: 8 passed, 42 skipped | 3 秒 |
| dispatcher_cex | `cargo nextest run -p task-dispatch -E 'test(cex_) \| test(write_set_) \| test(store_access_guard) \| test(structure_guard)'` | 0 | 67 tests run: 67 passed, 1854 skipped | build 込み 103 秒 |
| repo_health_tests | `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'` | 0 | Ran 126 tests、OK | 52.5 秒 |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | — | 53 秒 |
| fmt | `cargo fmt --all -- --check` | 0 | 出力 0 行 | 9 秒 |

check 1 は `--status-level pass --final-status-level pass` を付けて走らせ、各試験の PASS の行を記録した。16 件の内訳:

| key | 試験 | 結果 |
|---|---|---|
| case1 | `boundary_case_1_authz_one_line_is_high` | PASS 3.77 秒 |
| case2 | `boundary_case_2_equivalent_plain_change_is_low` | PASS 3.00 秒 |
| case3 | `boundary_case_3_move_cannot_escape` | PASS 2.84 秒 |
| case4 | `boundary_case_4_worsening_dependency_blocks` | PASS 2.09 秒 |
| case5 | `boundary_case_5_policy_self_approval_impossible` | PASS 1.67 秒 |
| case6 | `boundary_case_6_fail_closed` | PASS 11.01 秒 |
| prompt_boundary | task-worker `change_rules::review::tests` の `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule`・`prompt_boundary_design_review_shows_reason_owner_row_and_invariants`・`prompt_boundary_final_review_shows_finding_symbol_rationale_and_owner_row` | 3 件 PASS |
| prompt_full_objective | task-worker `claude_code::tests::planner_prompt_tests::prompt_full_objective_planner_keeps_objective_and_acceptance_tails`、task-dispatch `health_review::design::tests::prompt_full_objective_design_review_keeps_objective_tail`・`review::tests::prompt_full_objective_final_review_keeps_objective_tail_and_every_criterion`・`review::tests::prompt_full_objective_final_review_run_passes_the_tail_to_the_reviewer` | 4 件 PASS |
| planner_prompt_acceptance | task-worker `claude_code::tests::planner_prompt_tests::planner_prompt_acceptance_tree_planner_lists_every_criterion`・`planner_prompt_acceptance_flat_planner_and_plan_kinds_list_every_criterion`・`planner_prompt_acceptance_reaches_every_harness_input` | 3 件 PASS |

dispatcher_cex の 67 件の内訳（`cargo nextest list` で数えた）: `cex_` 40（review_authority 19・lease_restart 12・late_completion 6・same_tick 3）、`write_set_` 13、`structure_guard` 14（`store_access_guard` 3 を含む）。
org_write_contract の 6 件と tx_boundary_contract の 8 件の試験名は、成果物 dir の `org_write_contract.log`・`tx_boundary_contract.log` にある
（例: `cos_org_writes_commit_row_browser_event_operation_and_audit_together`、`cos_record_writes_audit_and_reference_in_one_transaction`、`busy_store_fails_record_without_residue_and_retry_succeeds`）。

前回（H の gate.md・safety.tsv）との件数の比較:

| 項目 | 前回 | 今回 |
|---|---|---|
| boundary_case_ | 6 passed | 6 passed |
| prompt_boundary_ | 3 passed | 3 passed |
| prompt_full_objective_・planner_prompt_acceptance_ | 試験が無かった | 4 + 3 passed |
| repo-health の python 試験 | 126 OK | 126 OK |
| org_write_contract・tx_boundary_contract・task-dispatch の filter | 6・8・67 passed | 6・8・67 passed |

## 3. 本物の検査器に当てた probe

検査器の起動は dispatcher の health の関門と同じ引数の組（`--repo --base --head --policy-version 1 --out`。`health_review/gate.rs` の `run_report`）で、
head だけを `worktree` にした。profile は指定せず、repo の `profiles/default.toml`（`extends = "celeris"`）が選ばれた。前回の gate.md §3 と同じ手順である。

```sh
B=$(git merge-base HEAD main)
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out <report.json>
```

probe を入れた file は前回と同じ `crates/task-api/src/lifecycle.rs`。R2・R5 の後、この file の `use crate::cos::operations::{Applied, OperationAudit};` は無くなり、
HTTP の道具の import は `crate::http_io` に変わった。probe を差し込む `use crate::human_epoch::HumanEpoch;` の行は残っているので、前回と同じ `sed` で入れた。

| run | exit | verdict | findings | new | worsened | existing | deps.cycle | deps.violation | 時間 |
|---|---|---|---|---|---|---|---|---|---|
| probe なし（参考） | 0 | pass | 695 | 0 | 0 | 695 | pass（existing 17） | pass（0 件） | 102 秒 |
| probe a（probe_scc_edge） | 1 | fail | 696 | 1 | 0 | 695 | fail（existing 17 + new 1） | pass（0 件） | 110 秒 |
| probe b（probe_plain） | 0 | pass | 695 | 0 | 0 | 695 | pass（existing 17） | pass（0 件） | 116 秒 |

probe なしの run は、前回は差分が docs だけで `deps.*` が not_applicable だった。今回は R1〜R7 の crates/ の差分があるので `deps.*` も評価され、21 規則がすべて pass だった。

### 3.1 probe a（probe_scc_edge）

```diff
@@ -23,6 +23,7 @@ use time::OffsetDateTime;
 use crate::handlers::parse_project_id;
 use crate::http_io::{ApiResult, Params, json_response, no_query, read_json};
 use crate::human_epoch::HumanEpoch;
+use crate::knowledge::KnowledgeItem;
 use crate::middleware::require_admin;
 use crate::problem::{ApiProblem, ops_problem};
 use crate::query::parse_task_id;
```

- fail の規則は `deps.cycle` だけだった。new の finding:
  - rule_id `deps.cycle`、severity `error`、delta `new`、scope `module` / `crates/task-api/src/lifecycle`
  - fingerprint: `deps.cycle:` の後ろが 16 桁の hex。4 桁ずつ区切ると `a106-75aa-c992-c815` で、前回と同じ値（区切らない値は成果物 dir の `health-probe-scc-edge.json` にある）。
  - evidence の symbol: `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`、`crates/task-api/src/lifecycle.rs:26` `use crate::knowledge::KnowledgeItem;`
  - rationale: 「head の 49 単位の循環の内部に新しい組 crates/task-api/src/lifecycle -> crates/task-api/src/knowledge（行 1 本）。base ではこの組は循環の内部に無かった。節点の数が変わらなくても循環の内部の依存が増えた。」
- 単位の数が変わらない悪化を、B が足した組の finding が止めた。task-api の SCC の単位の数は、検査器の rationale では head でも 49 だった（SCC と内部辺の数え直しは並行の rc-deps が行う）。

### 3.2 probe b（probe_plain）

```diff
@@ -23,6 +23,7 @@ use time::OffsetDateTime;
 use crate::handlers::parse_project_id;
 use crate::http_io::{ApiResult, Params, json_response, no_query, read_json};
 use crate::human_epoch::HumanEpoch;
+// probe: comment only (gate-cases)
 use crate::middleware::require_admin;
 use crate::problem::{ApiProblem, ops_problem};
 use crate::query::parse_task_id;
```

- 21 規則がすべて pass。`deps.cycle`・`deps.violation` の new・worsened の finding は出なかった。

## 4. 戻したことの確認

- probe はそれぞれ検査器を走らせた直後に `git checkout -- crates/task-api/src/lifecycle.rs` で戻し、commit していない。
- 戻した後の `git status --porcelain -- crates scripts web` は 2 回とも空だった。
- 提出時の差分は `agent-docs/progress/2026-11-19-arch-v4-h/replan/gate.md`・`gate.tsv` の 2 file だけである（§5）。

## 5. 範囲の確認

```sh
git status --porcelain -- crates scripts web                     # 空
git diff --name-only "$CELERIS_WU_BASE" -- crates scripts web    # 空
sh "$CELERIS_WU_SCOPE_PATHS"                                     # replan/gate.md・replan/gate.tsv だけ
```

## 6. 回帰条件

- `boundary_case_` 6 件、`prompt_boundary_` 3 件、`prompt_full_objective_` 4 件、`planner_prompt_acceptance_` 3 件、`test_repo_health_*.py` 126 件、
  org_write_contract 6 件・tx_boundary_contract 8 件・task-dispatch の filter 67 件が通り続けること。
- 本物の検査器に §3.1 の probe を当てると、`deps.cycle` の new の組 finding（symbol `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`）が出て exit 1 になること。
  main で `lifecycle -> knowledge` の組が SCC の内部にできた後は、この probe は使えない。そのときは前回の gate.md §3 の手順で、まだ無い組を選び直す。
- §3.2 の probe（comment 1 行）で、`deps.*` の new・worsened が 0 件のまま exit 0 になること。

## 7. 再現の手順

```sh
cargo nextest run -p task-dispatch -p task-worker -E 'test(/boundary_case_|prompt_boundary_|prompt_full_objective_|planner_prompt_acceptance_/)' --status-level pass --final-status-level pass
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract
cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract
cargo nextest run -p task-dispatch -E 'test(cex_) | test(write_set_) | test(store_access_guard) | test(structure_guard)'
python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'
cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
B=$(git merge-base HEAD main)
F=crates/task-api/src/lifecycle.rs
# probe a
sed -i 's/^use crate::human_epoch::HumanEpoch;$/&\nuse crate::knowledge::KnowledgeItem;/' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/a.json; echo $?   # 1
git checkout -- $F
# probe b
sed -i 's|^use crate::human_epoch::HumanEpoch;$|&\n// probe: comment only (gate-cases)|' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/b.json; echo $?   # 0
git checkout -- $F
```

生の記録（この run の成果物 dir）: `check1.log`、`org_write_contract.log`、`tx_boundary_contract.log`、`dispatcher_cex.log`・`dispatcher_cex.list`、
`repo_health_tests.log`、`clippy.log`、`fmt.log`、`health-clean.{json,log}`、`health-probe-scc-edge.{json,log}`、`health-probe-plain.{json,log}`、
`probe_scc_edge.diff`、`probe_plain.diff`。
