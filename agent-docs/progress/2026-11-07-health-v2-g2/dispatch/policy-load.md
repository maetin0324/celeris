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
件名「G2 policy-load: base 側の検査器で review 方針を読み planner の RunContext に渡す」の commit、件名「G2 policy-load: 再起動後の統合やり直しの試験で d1 の統合の終わりを待つ（負荷で merge が重なり Blocked になる）」の commit（2 回目の run。下の「前回の check の不合格」）と、この進捗の commit。
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

## 前回の check の不合格（2 回目の run）
1 回目の run の後に celeris が走らせた check（`bash scripts/dev/test-affected.sh && cargo clippy … && cargo fmt … --check`）は exit 100 だった。原因はこの WU の変更ではなく、負荷で落ちる時間依存の試験 2 件。どちらも planner を起こさないので、この WU の唯一の挙動の変更（planner の文脈の `review_policy`）を通らない。
- `celeris-campaign` の `launcher::a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid`（celeris の check で失敗）: 単独では通る（8 秒）。推定の原因（未確認）: `scripts/campaign/measure-launcher.sh` の health 待ちが `owned_alive`（`/proc/<pid>/cmdline` に `--config` を含むか）を 0.1 秒ごとに見ており、nohup → wrapper → 試験 binary の exec の途中で cmdline を読み損ねると「daemon が落ちた」と判定して片付ける（失敗の出力「pid … is not the daemon this launcher started … daemon exited before becoming healthy」、daemon.log は空）。`scripts/campaign/` はこの WU の範囲 check の外なので直していない（下の「提案」）。
- `task-dispatch` の `dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`（2 回目の run の再実行で失敗。`Blocked` ≠ `Done`）: 単独では 5 回とも通る。原因: 試験は d1 を捨てて固定の 200 ms 待つだけで、d1 が spawn した統合（merge）は捨てた後も走り続ける。負荷で d1 の merge が d2 のやり直しと同じ worktree で重なると、d2 の merge が `index.lock` で失敗し、`integration_gives_up` で `Blocked` になる。決定的な再現: PATH の先頭に置いた `git` の wrapper が最初の `git merge` の前に worktree の `index.lock` を 1.5 秒持つ（merge 中の d1 を模す）と、直す前は 2 回とも同じ行（work_units.rs の `assert_eq!(…status, Status::Done)`）で失敗した。直し方: d1 を捨てる前に `d1.integrating` から統合の `JoinHandle` を取り出して終わりを待つ（落ちたデーモンの統合はデーモンと一緒に止まる。固定の sleep は消した）。直した後は wrapper ありで 3 回、なしで 3 回とも通る。

## 証拠（提出時の HEAD で実行）
この WU では main の取り込み・rebase をしていない。下の検査は最後の code の commit（件名「G2 policy-load: 再起動後の統合やり直しの試験で …」）の後に実行した（後の commit は進捗の file だけ）。
- `bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`（前回不合格の check と同じ文字列、同じ cwd）→ exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。check-health は verdict=pass・new=0・worsened=0（existing 718）、repo_health_gate の試験は全部通過、nextest は 3511 passed・11 skipped、doc test も通過。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許可の正規表現で除く）→ 範囲外の path なし（変更は `crates/task-dispatch/` の 13 file と `docs/architecture-map.md`、この進捗）
- `cargo nextest run -p task-dispatch -E 'test(health_review_policy_)'` → exit 0、12 passed（1463 skipped）
- `cargo nextest run -p task-dispatch -E 'test(an_interrupted_integration_is_redone_idempotently_after_restart)'` → wrapper あり 3 回・なし 3 回とも 1 passed
- `python3 scripts/dev/check-architecture-map.py` → exit 0（939 件の path を確認）
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
- 方針を読むのは task の先頭の repo（worker の cwd）だけ。複数の repo を持つ task の 2 つ目以降の repo の方針は planner に渡らない（`ExecutionPlannerContext.review_policy` が 1 つのため）。
- git の repo の無い task は `Absent` にした（`review_policy_absent` で risk は `none`）。`Unavailable` にすると、repo を触らない task がすべて設計 review に回る。
- design-gate は計画の採用時に `ReviewPolicyState` を要る。planner run の起動時に読んだ状態は保存していないので、design-gate は `Dispatcher::planner_review_policy` をもう一度呼ぶか、状態の受け渡し方を決める必要がある（この unit では決めていない）。
- 本物の `--print-review-policy` は main（integrate 01M6VVB2X5）にあり、この WU の基準にはまだ無い。dispatcher の試験は stub で確かめた。本物の script との結合は統合後の scenario で確かめる。
- `measure-launcher.sh` の起動直後の誤判定（上の推定の原因）は直していない。affected の試験に `celeris-campaign` が入る WU では、負荷の高いときに同じ試験が落ちうる。
- 本番で統合中にデーモンが kill されると、残った `git` の process か残った `index.lock` が新しいデーモンの統合のやり直しを `Blocked` にしうる（未確認。試験の直しは同じ process の中の重なりだけを扱う）。

## 提案
- `scripts/campaign/measure-launcher.sh` の health 待ちで、死んだ判定を「`/proc/<pid>` が無い、または状態が Z・X」に限り、identity（starttime・cmdline・実行像）の照合は health が答えた後の 1 回にする（exec の途中の cmdline で誤判定しない）。試験は exec を遅らせる wrapper（`sleep` してから `exec`）で決定的に再現する。`scripts/campaign/` を範囲に持つ別の task で扱う。
- design-gate で `Unavailable` の理由を event（`DesignReviewRequested.risk.reasons`）に残すとき、この unit の理由の文（`base <sha12> に scripts/dev/repo-health/repo_health.py が無い` など）をそのまま使う。
