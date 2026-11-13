---
title: 健全性v2 R5 guard — 試験の case 数・不変条件の試験の検査と mutation の台本
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 guard — 試験の case 数・不変条件の試験の検査と mutation の台本

完了日: 2026-11-10

WorkUnit `guard`。R5 の後の unit（束ね・重複の整理）が試験を消すときの機械の検査を作る。repo のコード（`crates/`・`scripts/`）は変えていない。基準は `$CELERIS_WU_BASE`（slices-check は `git merge-base HEAD main`）。

## 変更
- 件名「R5 guard: 試験の case 数と不変条件の試験を base と比べる cases.py と mutation の台本 mutate.sh」の commit
  - [cases.py](cases.py): `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base <rev> [--json]`
  - [mutate.sh](mutate.sh): `sh agent-docs/progress/2026-11-10-health-v2-r5/mutate.sh <patch> <crate> <nextest filter>`
  - [test_guard.py](test_guard.py): 合成の fixture（`$TMPDIR` の一時 git repo と偽の `cargo`）で両方を確かめる unittest 16 件
  - この file

## cases.py の決まり
- 数える物: `crates/<dir>/` の `.rs` の試験関数（`#[test]`・`#[tokio::test(...)]`・`#[<path>::test]`・`#[rstest]`・`#[test_case(...)]`）と、表で回す case の行（試験関数の本体の `for <pat> in [...]`・`&[...]`・`vec![...]`、同じ本体の `let x = [...]` を回す `for <pat> in x`、`#[case(...)]`・`#[test_case(...)]`）。comment と文字列の中（例 `crates/campaign/tests/grader_independent.rs` の fixture の `#[test]`）は数えない。`macro_rules!` の `fn $name` は、規則が 1 つで matcher が `$x:frag` の並びなら呼び出しの引数から名前を引く（`crates/campaign/tests/dagbench_overhead.rs` の `point_test!`、`crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs` の `bench_case!`）。
- 同一性: crate の中の関数名の多重集合。file と module path は見ない。束ねで `tests/a.rs` を `tests/bundle/a.rs` に移しても消えたと数えない。表の行は関数名ごとの合計で比べる。
- base は `git ls-tree`・`git cat-file --batch` で `<rev>` から読む。作業ツリーは `git ls-files -co --exclude-standard` の file（未 commit・未追跡も含む）。`--crate` は dir 名か package 名（`celeris-campaign` → `crates/campaign`）。
- exit 1 になる条件:
  1. 減った関数（名前ごとの数の減り）・表の行（関数が残っている名前の行の合計の減り）が `removals/<dir>.json` に無い。
  2. removals の項目が不正: `reason` が空、`replacement` の試験が作業ツリーに無い（`<name>` は同じ crate、`<crate>:<name>` は他の crate）、`mutation.result` が `killed` でない、`mutation.patch` が作業ツリーに無い、`mutation.filter` が無い。
  3. B0 の `capability-map.json` の `capabilities[].tests` と `invariants[].evidence_path`、それらが `#[cfg(test)] [#[path = ".."]] mod x;` で読む子 file にある base の試験関数が、作業ツリーで減った。removals に載せても通さない（表の行を減らす `cases` の項目だけは載せられる）。
- exit 2: 引数・git・JSON の誤り、capability-map が無い。
- removals の形は [cases.py](cases.py) の docstring。項目は `{"name", "count"?, "cases"?, "reason", "replacement": [...], "mutation": {"patch", "filter", "result": "killed"}}`。
- 作業ツリーで減っていない名前の項目は note として出す（失敗にはしない）。

## mutate.sh の決まり
- patch を `git apply --check` で確かめてから当て、`cargo nextest run -p <crate> -E <filter>` を背景で走らせて `wait` する。正常終了・失敗・INT/TERM/HUP のどれでも `git apply -R` で戻し、戻した後の対象 file の hash が当てる前と同じかを確かめる。中断では試験の process を TERM で止めてから戻す。
- 判定: nextest の exit 100 → `KILLED`（exit 0）、exit 0 → `SURVIVED`（exit 1）、それ以外 → `ERROR`（exit 2）。戻せない・内容が違う → exit 3。中断 → 130/143/129。
- 依頼文は「失敗すれば KILLED」だが、build の失敗（101）・試験 0 件（4）・filter の誤り（94）は KILLED にしない。compile しない mutation や filter の書き損じは、残した試験の検出力の根拠にならないため。exit code は cargo-nextest 0.9.146 で小さい crate を作って確かめた（通過 0・試験の失敗 100・build の失敗 101・試験 0 件 4・filter の誤り 94）。

## 試験
| 試験 | 守ること |
|---|---|
| `CasesTest.test_unchanged_worktree_is_zero` | base と同じ作業ツリーで exit 0。関数 8・表の行 5・不変条件の file 3（evidence の子 file を含む）を数える。comment・raw string・文字列の中の `#[test]` を数えない |
| `CasesTest.test_package_name_resolves_to_dir` | package 名で dir を引く |
| `CasesTest.test_bundling_move_is_not_a_removal` | `tests/api.rs` を `tests/bundle/main.rs` + `mod api;` へ移しても exit 0 |
| `CasesTest.test_protected_file_moved_by_name_passes` | 不変条件の試験の file を別 file の inline module へ移しても exit 0 |
| `CasesTest.test_removed_function_without_entry_fails` | 関数を消すと exit 1 |
| `CasesTest.test_removed_function_with_valid_entry_passes` | 正しい removals の項目があれば exit 0 |
| `CasesTest.test_invalid_entries_fail` | 理由なし・代わりが無い・survived・patch なし・filter なしは exit 1 |
| `CasesTest.test_removed_table_rows_need_cases_entry` | 表の行を 2 行減らすと exit 1、`cases: 1` でも exit 1、`cases: 2` で exit 0 |
| `CasesTest.test_protected_function_cannot_be_removed_even_with_entry` | evidence の子 file の試験・macro で作る試験を消すと、removals があっても exit 1 |
| `CasesTest.test_bad_base_is_usage_error` | 無い rev・無い crate は exit 2 |
| `MutateTest.test_failing_tests_mean_killed_and_patch_is_reverted` | exit 100 → KILLED（0）。nextest に渡す引数と、試験が patch を当てた木で走ったことを確かめ、戻ったことを確かめる |
| `MutateTest.test_passing_tests_mean_survived_and_patch_is_reverted` | exit 0 → SURVIVED（1）。戻る |
| `MutateTest.test_build_failure_is_error_not_killed` | 101・4・94 → ERROR（2）。戻る |
| `MutateTest.test_patch_that_does_not_apply_changes_nothing` | 当たらない patch は exit 2 で、cargo を呼ばず file を変えない |
| `MutateTest.test_interrupt_reverts_patch` | TERM・INT・HUP で 143・130・129 で終わり、patch が戻り、試験の process も止まる |
| `MutateTest.test_usage_errors` | 引数の不足・無い patch は exit 2 |

unittest が台本の欠陥を検出できることを、台本を一時的に壊して確かめた（確かめた後は元に戻した）:
- mutate.sh の `trap on_exit EXIT` を消す → `test_interrupt_reverts_patch` の 3 件が落ちる。
- mutate.sh の正常経路の `restore` を消す → `MutateTest` の 5 件が落ちる。
- cases.py の消えた関数の集計を空にする → `test_removed_function_without_entry_fails` が落ちる。
- cases.py の不変条件の試験の照合を止める → `test_protected_function_cannot_be_removed_even_with_entry` が落ちる。

## 実 repo での結果
- 14 crate すべてで `cases.py --crate <c> --base "$CELERIS_WU_BASE"` が exit 0（`--base HEAD` でも exit 0）。数えた試験関数は campaign 993・celeris 806・celeris-credentiald 83・celeris-mcp 68・celerisctl 205・jev-classifier 44・llm-proxy 70・routing-eval 30・script-runtime 497・task-api 855・task-core 1351・task-dispatch 1661・task-ops 671・task-worker 1281。1 crate 0.3〜4.3 秒。
- `grep '^\s*#\[\s*([A-Za-z_]+::)*test\b'` の行数と比べた。差は文字列の中の fixture（campaign 2 件）、macro の定義の属性と呼び出しの展開（campaign `point_test!` 7 件、task-dispatch `bench_case!` 10 回 × 4 件）、rstest（script-runtime の vendor 4 件）で、すべて説明がつく。
- R1 の result-contract の前の commit（件名「R1 result-contract: result.json の解釈を result_report.rs の結果契約 1 つにし、5 adapter の写しを消す」の親）を base にして task-worker を見ると exit 1 で、R1 が消した不変条件の試験 5 名（`invalid_result_file_json_is_retryable_error`・`malformed_evidence_in_result_file_does_not_fail_the_run`・`pi_adapter_result_question_yield_and_invalid_json`・`question_in_result_file_blocks_task`・`result_yield_becomes_terminal_yielded`）を挙げた。R5 の base は R1 の後なので、R5 の unit には影響しない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、slices-check は `git merge-base HEAD main`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/test_guard.py` → exit 0（Ran 16 tests、OK）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <14 crate のそれぞれ> --base "$CELERIS_WU_BASE"` → すべて exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health verdict=pass、new=0・worsened=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh` → exit 0（doc-only）
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `agent-docs/progress/2026-11-10-health-v2-r5/` の 4 file だけ

## 未解決
- 表の行は試験関数の本体の中の配列だけを数える。module の `const CASES: [..] = [..]` を回す表（例 `crates/campaign/tests/long_horizon_public_contract.rs`）と、JSONL 等の外部 file の case は数えない。後の unit がそれらを減らすときは removals に自分で書く。
- 同名の試験関数が crate の中に複数あるとき、片方を消して別の場所に同名を足すと、数は減らないので検出しない。
- 表の行を別の試験関数に分けると、行の減りとして removals の項目（mutation 付き）が要る。

## 提案
- 束ね・重複の整理の unit（bundle-*・dedupe-*）の check に `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base "$CELERIS_WU_BASE"` を置く。統合では base を `git merge-base HEAD main` にする。
- removals の `mutation.patch` は `agent-docs/progress/2026-11-10-health-v2-r5/mutations/<crate>-<name>.patch` に置き、`mutate.sh` の出力（`KILLED: ...`）を進捗に写す。
