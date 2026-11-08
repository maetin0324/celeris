---
title: 健全性v2 G2 dispatch policy-load — base 側の検査器で review 方針を読み planner の RunContext に渡す
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch policy-load — base 側の検査器で review 方針を読み planner の RunContext に渡す

完了日: 2026-11-08

WorkUnit `policy-load`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D2.3・I3。

## 変更
件名「G2 policy-load: base 側の検査器で review 方針を読み planner の RunContext に渡す」の commit と、この進捗の commit。
- `crates/task-dispatch/src/health_review.rs`（新しい top-level module）: 下位 module `policy`・`design`・`input`・`verdict` を宣言。`design.rs`・`input.rs`・`verdict.rs` は後続の leaf（design-gate・health-input・health-verdict）のための module doc だけの file。
- `crates/task-dispatch/src/health_review/policy.rs`:
  - `load_review_policy(repo, target, timeout) -> ReviewPolicyState`: `git rev-parse --verify <target>^{commit}` で target を sha にし、`git cat-file -e <sha>:scripts/dev/repo-health/repo_health.py` で base に script があるかを確かめ、`git archive <sha> -- scripts/dev/repo-health [scripts/dev/source-size-report.py]`（side の file は base にあるときだけ）を一時 dir（`$TMPDIR/celeris-review-policy-<ulid>`、drop で消す）に展開し、`python3 -I -B <一時 dir>/scripts/dev/repo-health/repo_health.py --print-review-policy --repo <repo> --base <sha>` を起こす。候補（head・作業ツリー）の検査器と profile は使わない。
  - 全体（ref の解決・archive・展開・script）に 20 秒の上限（`REVIEW_POLICY_TIMEOUT`）。子 process は既存の `task_ops::changes::{git, run_with_timeout}`（process group ごと kill）で起こす。
  - 出力の写し（`parse_review_policy`）: `source = absent` → `Absent`、`base`・`builtin` → `Present`。base に script が無い・target を解決できない・非 0 exit（stderr の最初の行と exit code）・壊れた JSON・schema 違い・未知の欄（`deny_unknown_fields`）・`definition_sha256` が 64 桁の 16 進でない・時間切れ・`source = bootstrap`・未知の source → `Unavailable(理由)`。
  - `Dispatcher::planner_review_policy(task)`: remote workspace → `Unavailable("remote workspace")`。task の先頭の repo（worker の cwd）が git の worktree でなければ `Absent`（方針を読む repo が無い）。target は review 前の同期と同じ `review_target_ref`（木の子は親の branch、ほかは repo の既定 branch）。案件の repo の無い task は worktree の base。source の repo（`LocalWorktree.repo`）で読むので、worktree がまだ無い最初の planner run でも読める。`Unavailable` は理由を `tracing::warn` に残す。
- `crates/task-dispatch/src/dispatcher/planner_flow.rs`: `execution_planner_context` の `review_policy: None`（と配線待ちの注記）を `self.planner_review_policy(task).policy().cloned()` に置き換えた（2 行を 2 行に。`Present` のときだけ `Some`）。この関数は planner run の起動時（`dispatch_run.rs` の `is_planner_dispatch`）に 1 回だけ呼ばれ、/1・/2 と /3（`tree` 入り）の planner で共通。
- `dispatcher/workspaces.rs` の `task_workspaces_for` と `dispatcher/behind_target.rs` の `review_target_ref` を `pub(super)` から `pub(crate)` にした（行数は変わらない）。
- `crates/task-dispatch/src/lib.rs`: `pub mod health_review;`（doc 1 行と合わせて 2 行）。
- `docs/architecture-map.md`: task-dispatch の節に「健全性の review」の行（entry point・関連試験・不変条件の列）。
- 試験（prefix `health_review_policy_`、12 件）:
  - `crates/task-dispatch/src/health_review/policy/tests.rs`（9 件）: 一時 git repo に stub の `repo_health.py`（隣の `profiles/review.json` を出す）を commit。Present の全欄の写し（`--base` が解決した sha であること、side の file も出ること）、builtin、absent、非 0 exit、壊れた JSON・schema 違い・未知の欄・不正な digest、bootstrap と未知の source、base に script が無い（作業ツリーにはある）・解決できない target、時間切れ（2 秒の上限で 120 秒眠る stub を殺す）、base と候補・作業ツリーの比較。
  - 比較の試験 `health_review_policy_base_side_wins_over_candidate_and_worktree`: main に stub A と profile（class `base-class`）を commit、branch `candidate` で stub と profile を書き換えて commit（class `candidate-class`）、さらに作業ツリーで profile（`worktree-class`）と stub（exit 7）を書き換える。target が main の sha・`main` なら `base-class`、target を `candidate` にしたときだけ `candidate-class`。
  - `crates/task-dispatch/src/dispatcher/tests/health_review_policy.rs`（3 件）: planner run の `RunContext.execution_planner.review_policy` に main の stub の方針が入る（source の repo の作業ツリーで stub を書き換えても base の版）・計画が採用されて WU が走る、/2 の planner（`tree = None`）と /3 の planner（`tree = Some`）の両方に同じ方針、`Absent`（source absent・git でない dir）と `Unavailable`（非 0 exit・remote）は文脈に入れない。

## size の制約
`crates/task-dispatch/src/dispatcher/` に足した本体の行は 0（`planner_flow.rs` は 2 行を 2 行に置き換え、ほかは可視性の語だけ）。新しい処理は `health_review/` に置いた。`check-health.sh` は verdict=pass・new=0・worsened=0。

## 証拠（提出時の HEAD で実行）
（下の「検査結果」節を参照）

## 未解決
- 方針を読むのは task の先頭の repo（worker の cwd）だけ。複数の repo を持つ task の 2 つ目以降の repo の方針は planner に渡らない（`ExecutionPlannerContext.review_policy` が 1 つのため）。
- git の repo の無い task は `Absent` にした（`review_policy_absent` で risk は `none`）。`Unavailable` にすると、repo を触らない task がすべて設計 review に回る。
- design-gate は計画の採用時に `ReviewPolicyState` を要る。planner run の起動時に読んだ状態は保存していないので、design-gate は `Dispatcher::planner_review_policy` をもう一度呼ぶか、状態の受け渡し方を決める必要がある（この unit では決めていない）。
- 本物の `--print-review-policy` は main（integrate 01M6VVB2X5）にあり、この WU の基準にはまだ無い。dispatcher の試験は stub で確かめた。本物の script との結合は統合後の scenario で確かめる。

## 提案
- design-gate で `Unavailable` の理由を event（`DesignReviewRequested.risk.reasons`）に残すとき、この unit の理由の文（`base <sha12> に scripts/dev/repo-health/repo_health.py が無い` など）をそのまま使う。
