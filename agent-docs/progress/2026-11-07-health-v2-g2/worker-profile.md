---
title: 健全性v2 G2 worker-profile — worker の run にも repo の review 方針（profile の [review] 節）を配る
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 worker-profile — worker の run にも repo の review 方針（profile の [review] 節）を配る

完了日: 2026-11-08

WorkUnit `worker-profile`。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の付記「二度目の final review の差し戻しへの方針」(b) B1〜B5 を実装した。二度目の final review の受け入れ条件 0 の (2)「worker の run には profile が出ない」への修正。基準は `$CELERIS_WU_BASE`。ADR は編集していない（付記との違いは下の節に書き、unit reclose が ADR に写す）。

## 変更
- commit「G2 worker-profile: review 方針を RunContext.workspace に移し、worker と planner の run に base 側から届ける」
  - protocol（`crates/task-worker/src/protocol.rs`）: `RunContext.workspace_note: Option<String>` を `RunContext.workspace: Option<RunWorkspaceContext>` に置き換えた。`RunWorkspaceContext { note, review_policy }` は新しい型。`ExecutionPlannerContext.review_policy` は消した。`RunContext` の直下の欄は 40 のまま。読み口は `RunContext::workspace_note()` と `RunContext::review_policy()` の 2 つ。互換の読み替えは置いていない。
  - task-worker: `change_rules::repo_section` と `design_brief_section` は `context.review_policy()` だけを読む（B4）。`design_brief_section` の引数は `&ExecutionPlannerContext` から `&RunContext` に変えた。旧い読み口 `change_rules::review_policy`（`execution_planner.review_policy` を読む関数）は消した。前置き（`preamble.rs`）と `claude_code/prompt.rs` は `workspace_note()` を読む。
  - dispatcher: `policy.rs` の `task_review_policy` を `review_policy_target(task) -> Result<Option<(PathBuf, String)>, String>` に変えた（B2。tick の中で target を決めるだけで、読み込みはしない）。`planner_review_policy` と、`execution_planner_context` の中の同期の読み込みは消した。新しい file `health_review/worker_profile.rs` は 2 つを持つ。`Dispatcher::run_workspace` は `run_extras` の中で作業場所の説明と target を `RunExtras.workspace` に入れる。`RunWorkspace::into_context` は `run_worker` の非同期部分で `spawn_blocking` を使って `load_review_policy` を 1 回呼ぶ（上限 `REVIEW_POLICY_TIMEOUT`）。`Unavailable` の理由は warn の log に残す（B5）。
  - schema: `docs/protocol/worker-protocol.schema.json` を再生成し、`docs/protocol/worker-protocol.md` の `workspace_note` の記述を `workspace` に直した。
- commit「G2 worker-profile: worker_profile.rs を dispatcher::run_context の子 module として読み、dispatcher から health_review への依存の辺を足さない」
  - `worker_profile.rs` の file は `health_review/` に置いたまま、`dispatcher/run_context.rs` から `#[path]` で子 module として読む。`health_review/design_gate.rs` と同じ形にした。
- commit「G2 worker-profile: 進捗に変更・付記との違い・検査の結果を書く」 — この進捗 file を足した。

### 消した物（旧い置き場所と読み口）
- 欄: `RunContext.workspace_note`、`ExecutionPlannerContext.review_policy`。
- 関数: `Dispatcher::planner_review_policy`、`Dispatcher::task_review_policy`、`change_rules::review_policy`（private）。
- 試験: `health_review_policy_reaches_the_planner_run_context`、`health_review_policy_absent_and_unavailable_stay_out_of_the_planner_context`、`health_protocol_planner_review_policy_round_trips`。新しい置き場所の試験（下記）に置き換えた。
- 確認: `grep -rn "planner_review_policy\|task_review_policy" crates/ --include=*.rs` → 0 件。`.workspace_note` の欄の参照は残っていない（`workspace_note()` は新しい読み口、`preamble::workspace_note` は説明の文を組む既存の関数）。

### 足した試験
- task-worker（`change_rules/worker_profile_tests.rs`）
  - `change_rules_worker_profile_once_in_worker_and_planner_prompts`: atomic の execute・WU の run・planner の run の実際の prompt（`build_prompt`）で、repo 固有の規約の見出しが 1 回だけ出て、汎用規約の直後・Objective より前にある。
  - `change_rules_worker_profile_absent_without_a_policy`: 方針の無い run では節が出ない。reviewer の run（`workspace` 無し）と CoS chat（`build_cos_chat_parts`）にも出ない。
- task-dispatch
  - `health_review_worker_profile_reaches_planner_and_work_unit_runs`: planner の run と 2 本の WU の run の `RunContext.workspace.review_policy` に、target（main）の検査器の方針が入る。source repo の作業ツリーの未 commit の検査器は使わない（I3）。`ExecutionPlannerContext` の JSON に `review_policy` の key が無い。
  - `health_review_worker_profile_reaches_the_atomic_run`: 計画の無い atomic task の worker の run に方針が入る。
  - `health_review_worker_profile_unreadable_policy_stays_out_of_the_run`: `source = absent`・非 0 exit の検査器・git の repo の無い作業場所・remote workspace では、run は止まらず `review_policy = None`。remote は `policy_target = Err("remote workspace")`。
  - `health_review_worker_profile_without_a_policy_keeps_only_the_note`（`health_review/worker_profile/tests.rs`）: 方針が読めなくても作業場所の説明は残る。

## 付記との違い（reclose が ADR に写す）
- file の置き場所: 付記は module の場所を決めていない。処理は `health_review/worker_profile.rs` に置き、`dispatcher::run_context` の子 module として `#[path]` で読む（`design_gate.rs` と同じ形）。理由は 2 つ。`dispatcher/` の module は本体の行が閾値 8,000 を超えている（約 35,000 行）。また `dispatcher` から `health_review::worker_profile` への依存の辺を足さないため。`dispatcher/` の非試験 file の差分は +7 −5 行で、増えた分は doc comment と `mod` 宣言。check-health の `crates/task-dispatch/src/dispatcher` の module 本体の行は `existing`（悪化なし）。
- 対話 run: 付記 B3 は「worker の run と planner の run」に入れると書き、対話 run（task の会話の宛先がある run）には触れていない。実装では対話 run に方針を入れない（`run_workspace` の `with_policy = conversation_addressee.is_none()`）。作業場所の説明は従来どおり入る。
- repair・integration_repair の run: `run_extras` の呼び出し口は `dispatch_run.rs` の 1 か所で、WU の repair も同じ経路を通る。repair の run だけを対象にした試験は置いていない。
- 試験の範囲: 付記の `health_review_worker_profile_` は「`RunExtras` に target が入る、remote workspace では入らない」。実装の試験は、`RunExtras.workspace.policy_target` に加えて、adapter が受け取った `RunContext` まで確かめる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、check-health は `git merge-base HEAD main`）
- 条件 0（WU の run と atomic task の run の prompt に方針の節が出る）: `cargo nextest list` で `change_rules_worker_profile_` 2 件・`health_review_worker_profile_` 4 件を確認。どれも `test-affected.sh` の中で合格。
- 条件 1（base 側から読み、`RunContext` の直下の欄は増えていない）: `RunContext` の `pub` の欄は 40（`awk '/^pub struct RunContext \{/,/^\}/' crates/task-worker/src/protocol.rs | grep -cE '^\s+pub [a-z_]+:'` → 40）。check-health に `RunContext` の finding は無い。base 側の読み込みは `health_review_worker_profile_reaches_planner_and_work_unit_runs` が確かめる。
- 条件 2（旧い置き場所の欄・読み口が残っていない）: 上の「消した物」の grep → 0 件。
- 条件 3（schema が更新済み）: `docs/protocol/worker-protocol.schema.json` に `RunWorkspaceContext` がある（`grep -c` → 2）。`workspace_note` の欄は無い（`grep` の 1 件は `note` の説明文の中の `preamble::workspace_note` への言及）。
- 条件 4:
  - `bash scripts/dev/check-health.sh` → exit 0。`verdict=pass fail_grade=0 new=0 worsened=0 existing=718`。
  - `bash scripts/dev/test-affected.sh` → exit 0。nextest 7,963 件合格（23 件 skip）、check-health worsened=0、web の tsc -b と vitest 104 file・711 件合格、test-ts-verify 20 件合格。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。

## 未解決
- 方針は run の起動ごとに検査器（`git archive` と python）で読む。cache は置かない（付記 B2）。1 回の上限は 20 秒で、tick は止めない。run の起動が多い task では起動ごとに数百 ms 程度の読み込みが足される（実測していない）。
- 方針を読む repo は task の先頭の repo だけ（付記 B3）。複数の repo を持つ task の 2 つ目以降の repo の方針は、worker の prompt に出ない。

## 提案
- なし。
