---
title: アーキ v4 H gate-cases — C の 6 ケース等を再実行し、本物の検査器で SCC 内の新しい辺が止まり普通の変更が通るかを確かめる
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# gate-cases（WorkUnit）

対象の tree はこの WorkUnit の HEAD（件名 `integrate wu/struct-store (phase measure)`）。health の base は `git merge-base HEAD main`
（件名 `integrate wu/close (phase close)` の commit。アーキ v4 G までが入った main）。HEAD と base の差は H の measure の docs だけで、
`git diff --quiet "$(git merge-base HEAD main)" HEAD -- scripts/dev/repo-health` は 0 だった（検査器は base のものと同じ内容）。
各行は [gate.tsv](gate.tsv)（列 `case result command note`）。result は「期待どおりの結果だったか」で、検査器の verdict ではない
（危険な probe は検査器が fail を出せば pass）。host は home-dev、時刻は 2026-11-19T11:55Z〜12:05Z、開始時の 1 分 load は 1.66。
crates/・scripts/・web/ と policy の file は変えていない（probe は戻した。§4）。

## 0. 結論

- 10 行すべて期待どおりだった（未達 0）。
- C の 6 ケース（`boundary_case_1_`〜`6_`）は提出時の HEAD で 6 件とも通った。
- prompt の 3 件（`prompt_boundary_`）と repo-health の python 試験 126 件も通った。
- 本物の検査器に当てた probe の結果:
  - probe a: task-api の 49 単位の SCC の中に、まだ無い向きの組 `lifecycle -> knowledge` を 1 行足した。`deps.cycle` が fail になり（new 1 件）、検査器は exit 1 を返した。
  - probe b: 同じ file に comment を 1 行足した。`deps.cycle`・`deps.violation` の new・worsened は 0 件で、検査器は exit 0 を返した。
- 注意（未達ではない）: `prompt_boundary_` で 3,000 文字の objective が末尾まで載ることを確かめているのは worker の prompt だけである。
  planner と reviewer の prompt について、この試験が確かめるのは class の不変条件・owner 地図の行・finding の symbol までで、objective の末尾は確かめていない（§2）。
  実 prompt に objective が載ったかは、並行の WorkUnit prompt-audit が記録で確かめる。

## 1. 試験の再実行

| case | コマンド | exit | 件数 | 時間 |
|---|---|---|---|---|
| case1〜case6 | `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)'` | 0 | 6 tests run: 6 passed, 1912 skipped | 1 回目は build 1 分 39 秒込みで 1 分 52 秒。試験は 10.9 秒 |
| prompt_boundary | `cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)'` | 0 | 3 tests run: 3 passed, 1311 skipped | build 1 分 21 秒、試験 0.018 秒 |
| repo_health_tests | `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'` | 0 | Ran 126 tests、OK | 51.6 秒 |

結果の行を出すため、nextest は `--status-level pass --final-status-level pass` を付けてもう 1 回走らせた（同じ件数、exit 0）。各ケースの結果は次のとおり。

| case | 試験 | C の主張 | 結果 |
|---|---|---|---|
| case1 | `boundary_case_1_authz_one_line_is_high` | 認可の条件 1 行の変更は、`new_*` の宣言が無くても High で、設計 review を求める | PASS 3.40 秒 |
| case2 | `boundary_case_2_equivalent_plain_change_is_low` | 画面・class 外の handler の同等の変更は Low で、設計 review を求めない | PASS 2.58 秒 |
| case3 | `boundary_case_3_move_cannot_escape` | file の移動・別名・`#[path]` の付け替えで class の判定を逃れられない | PASS 2.46 秒 |
| case4 | `boundary_case_4_worsening_dependency_blocks` | 汎用 Rust の fixture で、既存の循環への新しい組は health の関門で止まる | PASS 1.74 秒 |
| case5 | `boundary_case_5_policy_self_approval_impossible` | 候補自身の policy で class を外しても、base の policy で判定される | PASS 1.45 秒 |
| case6 | `boundary_case_6_fail_closed` | 方針が読めない・差分が取れない・判定が欠けるときは合格にしない | PASS 10.93 秒 |

repo-health の試験には、B が足した SCC の内部辺の 5 件（`test_repo_health_scc_edges.py`）が入っている。内訳は新しい内部辺が fail・既存の内部辺は pass・辺の削除は pass・合法な型の import は pass・`super::` の path の新しい内部辺は fail。

## 2. prompt の試験が確かめる範囲

`crates/task-worker/src/change_rules/review/tests.rs` の 3 件が確かめる内容は次のとおり。

- `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule`: worker と planner の prompt の repo 固有の規約の節を確かめる。
  - 節に載るもの: 「`new_*` の宣言によらず設計 review の対象」の規則、class の path と不変条件の行、`deps.cycle` が fail になる依存の規則、`<src> -> <dst>` の書式。
  - objective は `"あ" × 3000 + "END-OF-OBJECTIVE"` にし、`worker.contains("END-OF-OBJECTIVE")` で末尾を確かめる。確かめるのは worker の prompt だけで、planner の prompt は確かめない。
- `prompt_boundary_design_review_shows_reason_owner_row_and_invariants`: 設計 review の prompt を確かめる。
  - risk の理由の行に、path・class・owner 地図の行・class の不変条件と根拠の file が載る。
  - design の判定が要る理由が出る。
- `prompt_boundary_final_review_shows_finding_symbol_rationale_and_owner_row`: final review の prompt を確かめる。
  - health の節に finding の `fingerprint`・`symbol`・`rationale` が載る。
  - 差分の設計の節に owner 地図の行が載る。

このため、この WorkUnit の Objective にある「reviewer の prompt に 3,000 文字の objective が末尾まで載る」は、この試験では確かめていない。
reviewer の prompt の objective の扱いは prompt-audit の記録で確かめる。prompt-audit で欠けが見つかれば、reviewer の prompt に長い objective を入れて末尾を確かめる試験を足す（次の候補）。

## 3. 本物の検査器に当てた probe

検査器の起動は dispatcher の health の関門と同じ引数の組（`--repo --base --head --policy-version 1 --out`。`health_review/gate.rs` の `run_report`）で、
head だけを Objective の指定どおり `worktree` にした。profile は指定せず、repo の `profiles/default.toml`（`extends = "celeris"`）が選ばれた。

```sh
B=$(git merge-base HEAD main)
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out <report.json>
```

単位の組は A §7 C2 の snippet（`deps._components` を base の tree に当てる）と同じ組み立てで数えた。task-api の SCC は 49 単位・261 組、task-core の SCC は 55 単位・210 組で、struct-deps.md の after と同じだった。
どちらの probe も task-api の SCC の単位 `crates/task-api/src/lifecycle` を使った。この単位から出る組は `cos`・`handlers`・`middleware`・`milestones`・`state` で、`knowledge`（同じ SCC の単位）への組は無い。

### 3.0 probe なし（参考）

exit 0、verdict=pass。変更は profile の doc_only の path だけなので、`deps.*`・size・dup は not_applicable（14.7 秒）。

### 3.1 probe a（probe_scc_edge）: SCC の中に、まだ無い向きの組を 1 行足す

```diff
--- a/crates/task-api/src/lifecycle.rs
+++ b/crates/task-api/src/lifecycle.rs
@@ -23,6 +23,7 @@ use time::OffsetDateTime;
 use crate::cos::operations::{Applied, OperationAudit};
 use crate::handlers::{ApiResult, Params, json_response, no_query, parse_project_id, read_json};
 use crate::human_epoch::HumanEpoch;
+use crate::knowledge::KnowledgeItem;
 use crate::middleware::require_admin;
 use crate::problem::{ApiProblem, ops_problem};
 use crate::query::parse_task_id;
```

- exit 1、verdict=fail（1 分 33.6 秒）。summary: findings 696、fail_grade 1、new 1、worsened 0、existing 695。
- `deps.cycle` は fail（findings 18 = existing 17 ＋ new 1）。`deps.violation` は pass（0 件）。他の規則はすべて pass。
- new の finding:
  - rule_id `deps.cycle`、severity `error`、delta `new`、scope `module` / `crates/task-api/src/lifecycle`
  - fingerprint: `deps.cycle:` の後ろが 16 桁の hex。この md では 4 桁ずつ `-` で区切って `a106-75aa-c992-c815` と書く（区切らないと `progress-index.sh --check` が commit の hash とみなして落ちる）。区切らない値は gate.tsv と run の成果物 dir の report にある。
  - symbol: `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`
  - evidence: `crates/task-api/src/lifecycle.rs:26` `use crate::knowledge::KnowledgeItem;`
  - rationale: 「head の 49 単位の循環の内部に新しい組 crates/task-api/src/lifecycle -> crates/task-api/src/knowledge（行 1 本）。base ではこの組は循環の内部に無かった。節点の数が変わらなくても循環の内部の依存が増えた。」
- 49 単位の SCC の finding 自身は existing のままで、単位の数は増えていない。単位の数が変わらない悪化を、B が足した組の finding（`cycle-edge:` の鍵）が止めた。

### 3.2 probe b（probe_plain）: 同じ file に comment を 1 行足す

```diff
--- a/crates/task-api/src/lifecycle.rs
+++ b/crates/task-api/src/lifecycle.rs
@@ -23,6 +23,7 @@ use time::OffsetDateTime;
 use crate::cos::operations::{Applied, OperationAudit};
 use crate::handlers::{ApiResult, Params, json_response, no_query, parse_project_id, read_json};
 use crate::human_epoch::HumanEpoch;
+// probe: comment only (gate-cases)
 use crate::middleware::require_admin;
 use crate::problem::{ApiProblem, ops_problem};
 use crate::query::parse_task_id;
```

- exit 0、verdict=pass（1 分 31.6 秒）。summary: findings 695、fail_grade 0、new 0、worsened 0、existing 695。
- `deps.cycle` は pass（findings 17、すべて existing。module の循環 17 と同じ数）。`deps.violation` は pass（0 件）。
- `deps.cycle`・`deps.violation` の new・worsened の finding は出なかった。他の規則もすべて pass。

## 4. 戻したことの確認

- probe はそれぞれ検査器を走らせた直後に `git checkout -- crates/task-api/src/lifecycle.rs` で戻し、commit していない。
- 戻した後の `git status --porcelain -- crates scripts web` は 2 回とも空だった。
- 提出時の差分は `agent-docs/progress/2026-11-19-arch-v4-h/gate.md`・`gate.tsv` の 2 file だけである（§5 の範囲の確認）。

## 5. 範囲の確認

```sh
git status --porcelain -- crates scripts web                     # 空
git diff --name-only "$CELERIS_WU_BASE" -- crates scripts web    # 空
sh "$CELERIS_WU_SCOPE_PATHS"                                     # gate.md・gate.tsv だけ
```

## 6. 回帰条件

- `boundary_case_` の 6 件、`prompt_boundary_` の 3 件、`test_repo_health_*.py` の 126 件が通り続けること。
- 本物の検査器に §3.1 の probe を当てると、`deps.cycle` の new の組 finding（symbol `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`）が出て exit 1 になること。
  ただし、main で `lifecycle -> knowledge` の組が SCC の内部にできた後は、この probe は使えない。そのときは §3 の手順で、まだ無い組を選び直す。
- §3.2 の probe（comment 1 行）で、`deps.*` の new・worsened が 0 件のまま exit 0 になること。

## 7. 再現の手順

```sh
cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)' --status-level pass --final-status-level pass
cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)' --status-level pass --final-status-level pass
python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'
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

生の記録（この run の成果物 dir）: `case1-6.log`、`prompt_boundary.log`、`repo_health_tests.log`、`health-clean.{json,log}`、
`health-probe-scc-edge.{json,log}`、`health-probe-plain.{json,log}`、`probe_scc_edge.diff`、`probe_plain.diff`。

## 8. 提案

- `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule` では、3,000 文字の objective の末尾を planner の prompt でも確かめていない。
  design review と final review の prompt の試験も、objective の末尾を確かめていない。prompt-audit の記録で欠けが見つかれば、reviewer と planner の prompt に
  長い objective を入れて末尾を確かめる試験を足す。起票するかは人が決める。
