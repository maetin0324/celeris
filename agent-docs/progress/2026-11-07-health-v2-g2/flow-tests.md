---
title: 健全性v2 G2 flow-tests — 本物の prompt を通した設計 review の流れ全体の試験
tasks: [01M6XS6T6GN5YKGCHFHH6CP5A4]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 flow-tests — 本物の prompt を通した設計 review の流れ全体の試験

完了日: 2026-11-08

[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の付記「二度目の final review の差し戻しへの方針」の「流れ全体の試験（flow-tests）」を実装する。前回の流れの試験（`health_review_scenario_high_risk_change`）は偽の reviewer が `review.json` に `design` を直接書くので、prompt の欠落を検出できなかった。この task の試験の偽の reviewer は、dispatcher が渡した `RunRequest` から本物の prompt を組み、その文面を確かめてから Result 節の例の JSON を書く。ADR は編集していない（付記との違いは下に書き、unit reclose が ADR に写す）。

この task は WorkUnit 2 つで進める。`prompt-flow`（高 risk・通常の変更。この節）と `remote-flow`（remote workspace の計画）。`remote-flow` の結果はその WorkUnit が書く。

# WorkUnit prompt-flow（高 risk・通常の変更）

## 変更
- commit「G2 flow-tests: 偽の reviewer が claude_code::build_prompt の prompt を確かめ Result 節の例の JSON を書く流れの試験（高 risk・通常の変更）を足す」
  - 新しい file `crates/task-dispatch/src/health_review/flow_tests.rs`。`dispatcher/tests/mod.rs` に `#[path = "../../health_review/flow_tests.rs"] mod health_review_flow;` を足した。
  - 偽の adapter `FlowAdapter` は、受けたすべての run で `task_worker::claude_code::build_prompt(&req.task, &req.context, run_id, &req.artifacts_rel())` を呼ぶ（claude_code adapter の本番の呼び方と同じ引数）。run の文脈と prompt を記録する。
    - reviewer の run（`context.review` が `Some`）は自分で受ける。
      - 設計 review: prompt に `design_section` の節があることを確かめる。確かめる値は `RunContext` から取る: 計画の要約、brief の 10 欄の見出し（`root_cause`〜`invariants`）と値（root_cause・owner・options の name/chosen/reason・new_state・removals）、risk の `level` と全 reasons、方針の class・paths・invariants（方針が無ければ「この repo の review 方針を読めなかった」の文）。設計 review の見出しがちょうど 1 回あり、受け入れ条件の判定の節（`## Criteria you must judge in this run`）が無いことも確かめる。
      - final review: 設計 review の節が無く、受け入れ条件の判定の節があることを確かめる。
      - Result 節（`## Result` の見出しの後で最初の、バッククォートで囲んだ `{…}` の 1 行）から例の JSON を取り出し、判定の値だけを差し替える。設計 review は `verdicts` が `[]` で `design` に `pass`・`findings` があることを確かめ、`design.pass = true`・`design.findings = []` にする。final review は例の verdict を雛形に criterion ごとに `pass = true` にして `repair` を除き、`health` の有無が `ReviewRequest.health` と同じことを確かめて `health.findings = []` にする。差し替えた JSON が `ReviewOutput` として読めることを確かめてから `review.json` に書く。
      - 確かめた結果が不足なら `review.json` を書かず、不足の内容を `problems` に残す（adapter の中で panic すると、失敗の理由が run の失敗に埋もれる）。試験は最初に `problems` が空であることを確かめる。
    - planner と WU の run は `health_review_scenario` の `ScenarioAdapter` に渡す（計画を書く・WU が write path に commit する処理を写していない）。
  - `crates/task-dispatch/src/health_review/scenario_tests.rs`: 補助を再利用できるようにした。`NORMAL_PATH`・`STORE_PATH`・`plan`・`ScenarioAdapter`（`new` と `seen`）・`done`・`Outcome`（`events`・`planner_runs`・`design_review_runs`・`work_unit_runs`・`position`・`status`）を `pub(super)` にした。`run_flow` の本体を `run_flow_with(with_policy, scenario, adapter)` に分けた（`run_flow` は `ScenarioAdapter` を作って `run_flow_with` を呼ぶ。既存の scenario 試験の挙動は変えていない）。
  - product code は変えていない。

## 足した試験（prefix `health_review_flow_`、2 件）
- `health_review_flow_high_risk_plan_is_adopted_after_the_prompt_review`: store の class の path（`crates/task-store/src/rows.rs`）を書き `new_state` を宣言した計画。設計 review の run が 1 回起き、prompt に試験の計画の値（`rows are rewritten in place`・`rows.revision column`・`` `append a column`（chosen=true） ``・`` - level: `high` ``・`` `store`: `^crates/task-store/` ``）が載る。`DesignReviewed { pass: true }` の後に `ExecutionPlanned`。planner の run は 1 回（再試行なし）、不合格の `DesignReviewed` は無い。`PlanApprovalRequested`・`PlanGateDecided`・`DecisionRequested` の event が無い。WU の run 1 回、final review 1 回で、task は `Done`。
- `health_review_flow_normal_change_puts_repo_rules_in_the_work_unit_prompt_only`: 高 risk の class に触れない計画（`crates/foo/src/lib.rs`）。設計 review の run は 0 回。WU の run は 1 回で、その `RunContext` に review 方針があり、prompt に repo 固有の規約の見出し（`change_rules::REPO_RULES_HEADING`）がちょうど 1 回出る。その節に方針の class（`` `store`: `^crates/task-store/` ``）が載る。final review の prompt にはその見出しが出ない。task は `Done`。
- 既存の scenario 試験が確かめる事（health の diff 統計・owner 行・blocking の finding で replan）は繰り返していない。

### 偽の reviewer が prompt の欠落を検出することの確認
`crates/task-worker/src/change_rules/review.rs` の `design_section` から `risk_lines` の呼び出しを手元で外し、`health_review_flow_high_risk_*` を走らせた。試験は落ち、`problems` に ``the design review prompt lacks "- level: `high`"`` と ``lacks "`path_class:store+new_state`"`` が出た。確認の後に `git checkout` で戻した（commit に含まない）。

## 証拠（提出時の HEAD で実行。check-health の基準は `git merge-base HEAD main`、範囲の基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --lib -E 'test(/health_review_flow_|health_review_scenario_/)'` → 5 件合格（flow 2・scenario 3）。
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は `verdict=pass new=0 worsened=0 existing=717`。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest は 3586 件合格・11 件 skip。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest list -p task-dispatch -E 'test(/health_review_flow_/)' --message-format oneline | grep -c health_review_flow_` → 2（task の条件の 3 件は WorkUnit `remote-flow` の 1 件と合わせて満たす）。
- `git diff --name-only $CELERIS_WU_BASE HEAD`（範囲）→ `crates/task-dispatch/src/dispatcher/tests/mod.rs`・`crates/task-dispatch/src/health_review/flow_tests.rs`・`crates/task-dispatch/src/health_review/scenario_tests.rs` と、この進捗 file。

## 付記との違い
- 試験名の prefix: 付記は `health_review_scenario_`、この task は `health_review_flow_` を使った（task の目的と受け入れ条件の check がこの prefix を数える）。既存の `health_review_scenario_` の 3 件はそのまま残した。
- prompt を組む関数: 付記は「本物の `build_review_prompt`」と書くが、この関数は `task_worker::claude_code::prompt` の private な関数で、試験から呼べない。試験は公開の入口 `task_worker::claude_code::build_prompt` を呼ぶ。reviewer の run の `req.task.kind` は `Review` なので、`build_prompt` は `build_review_prompt` に分岐する（claude_code adapter の本番の経路と同じ）。
- 付記の 3 つの場合のうち、remote workspace の場合は WorkUnit `remote-flow` が担当する。

## 未解決
- なし（この WorkUnit の範囲で product の欠陥は見つからなかった）。

## 提案
- なし。

# WorkUnit remote-flow（remote workspace）

## 結論
- 試験 `health_review_flow_remote_workspace_plan_is_adopted_after_one_design_review` を足した。remote workspace の task の計画は、設計 review の 1 run の合格で採用される。PlanGate・plan 承認の決定・人への受信箱の項目は起きない（付記 (d) D2 のとおり）。
- ただし今の Complexity Gate は remote workspace の task を最初の dispatch で atomic にする。remote の task で planner が走るのは、人が採用した計画（`PUT /tasks/{id}/execution-plan`）の WU が `yield.plan_issue` を返した後などの replan だけ。試験はこの本番の経路で planner を起こす。付記 (d) との違いと、人が決めることは下の「付記との違い」「未解決」に書いた。
- product code は変えていない。

## 経緯
- 1 回目と 2 回目の run は、gate が remote の task を atomic にして planner に届かないことを見つけ、product code を変えずに replan を求めた。
- 2 回目の run は `result.json` に `summary` と `yield` を両方書いた。`crates/task-worker/src/claude_code.rs` の result.json の読み取りは `question` > `summary` > `yield` の順なので、celeris はその run を done として扱い、計画の check（件数と grep）がまた落ちた。
- 3 回目（この run）は、remote の task が planner に届く本番の経路を探した。人が採用した計画の WU の `plan_issue` から replan が起きる経路（`dispatcher/tests/work_units.rs` の `a_plan_issue_checkpoint_triggers_a_replan_and_v2_is_adopted` と同じ仕組み）で試験を書いた。前回の下書きの前置き（`routing.execution` に人の compound の判定を前もって入れる。本番では作れない状態）は使っていない。

## 変更
- commit「G2 flow-tests remote-flow: remote workspace の task の replan の計画が設計 review の 1 run の合格で人の承認なしに採用される流れの試験を足す」
  - `crates/task-dispatch/src/health_review/flow_tests.rs`
    - `run_remote_flow`: task は `compound_task` で、workspace は `WorkspaceSpec::Remote { cluster: "sirius", path: "/work/x", mode: None }`。store に入れた後、`task_ops::execution::adopt_plan`（`PUT /tasks/{id}/execution-plan` と同じ関数、origin `Human`）で brief の無い計画（unit `a`、path `crates/foo/src/lib.rs`）を採用する。dispatcher にクラスタ `sirius` を設定し、ssh・rsync を `true` に差し替える（`session_resume` の remote の試験と同じ設定。外部ネットワークに出ない）。
    - `PlanIssueOnce`: 最初の WU の run だけ `Terminal::Yielded { checkpoint: {"plan_issue": …} }` を返し、ほかの run は `FlowAdapter` に渡す。replan の planner は brief 付きの計画（`plan(NORMAL_PATH, true)`）を出す。
    - `inbox_kinds_for`: `task_ops::human_inbox::human_inbox` の項目のうち、この task に触れるものの種類。
    - `flow_adapter`: `FlowAdapter` を作る補助（`run_prompt_flow` と共用）。
  - `crates/task-dispatch/src/health_review/scenario_tests.rs`: `Outcome` の欄を `pub(super)` にした（flow_tests.rs が組む）。`ScenarioAdapter` の WU の run は、作業ツリーが git でない（remote workspace の手元の写し）ときは file を書くだけにした（commit しない）。既存の scenario 試験の挙動は変えていない。

## 足した試験（prefix `health_review_flow_`、1 件。合計 3 件）
`health_review_flow_remote_workspace_plan_is_adopted_after_one_design_review` が確かめる事:
- `DesignReviewRequested` は 1 件で、`risk.level` は `Unknown`、reasons に `review_policy_unavailable:remote workspace` がある。
- 設計 review の prompt（本物の `claude_code::build_prompt`）に `` - level: `unknown` ``、「この repo の review 方針を読めなかった。理由: `review_policy_unavailable:remote workspace`。」、「設計 brief だけで判定する」、brief の値（`rows are rewritten in place`・`` `append a column`（chosen=true） ``）が出る。owner 地図の行は出ない。Result 節の例の JSON は `verdicts: []` で、`design` に `pass`・`findings` がある。`FlowAdapter` の prompt の検査（`design_prompt_problems`）も通る。
- `ExecutionPlanned` は origin `Human`（人の計画）、`Planner`（replan）の順に 2 件で、`DesignReviewed { pass: true }` はその間にある。不合格の `DesignReviewed` は無い。
- planner の run は 1 回（再試行なし）、設計 review の run は 1 回。
- `PlanApprovalRequested`・`PlanGateDecided`・`DecisionRequested` の event が無い。run が止まった後の人への受信箱に、この task の項目が無い。
- 採用の後（WU・final review）の挙動は確かめていない（Objective のとおり）。

## 証拠（提出時の HEAD で実行。check-health の基準は `git merge-base HEAD main`、範囲の基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --lib -E 'test(/health_review_flow_/)'` → 3 件合格（prompt-flow の 2 件とこの 1 件）。
- 計画の check: 件数 `health_review_flow_ tests: 3` の後の `cargo nextest run -p task-dispatch -E 'test(/health_review_flow_/)'` → exit 0。`grep -q 'WorkspaceSpec::Remote' … && grep -q 'fn health_review_flow_remote_workspace' …` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ `flow_tests.rs`・`scenario_tests.rs`・この進捗 file だけで、範囲外は無い。
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は `verdict=pass new=0 worsened=0 existing=717`。nextest は 3587 件合格・11 件 skip。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 付記との違い
- 付記 (d) D1〜D3 は、remote workspace の task が planner の計画を持つことを前提に書かれている。今の gate では、`crates/task-core/src/execution_gate.rs` の `out_of_scope_rule`（185 行）が remote の task を `atomic/out-of-scope` にする（`crates/task-core/src/model.rs:137` の `WorkspaceSpec::local_mode` が `Remote` に `Shared` を返す）。この規則は人の明示の compound（`execution_hint` の explicit）より先に効く。remote の task で planner が走るのは、人が採用した計画の replan だけ。試験はこの経路を使った。
- 付記 (d) D3 の「remote の task は計画ごとに reviewer の run が 1 回増える」は、この replan のときだけ起きる。
- 試験名の prefix は `health_review_flow_`（付記は `health_review_scenario_`。prompt-flow と同じ理由）。

## 未解決（人が決めること）
- remote workspace の task の最初の計画をどう扱うかが決まっていない。次の (A) か (B) を選ぶ必要がある。この WorkUnit はどちらも行っていない。
- remote の task に人が明示した compound は、gate で黙って atomic になる。`crates/task-ops/src/regate.rs` の `set_execution_mode`（人の再 gate・decompose の依頼）は remote の task への依頼を `Validation` で拒む。
- 同じ条件の 2 つ目の項 `remote_mode() == Shared`（`execution_gate.rs:186`）は、1 つ目の項が `Remote` を全部拾うので、どの task でも結果を変えない。「`Remote` のうち `mode: Shared` のものだけを対象外にする」意図だった可能性がある（ADR では確かめていない。未確認）。

## 提案
- (A) gate で remote workspace の task も compound にできるようにする。例: `out_of_scope_rule` の `Shared` の判定を `Local` の mode と `Remote` の `mode: Shared` だけに当てる（remote の WU は `parallel_mode` で既に並列 1 に落ちる）。`crates/task-core/` の変更なので、別の task にする。そのときは最初の計画の流れ（gate → planner → 設計 review）の試験を、この試験に足す。
- (B) remote workspace の task は atomic のままにし、付記 (d) D1〜D3 を「remote の task は人の計画の replan のときだけ planner の計画を持ち、そのときに設計 review が起きる」に改める。unit reclose が ADR に写す。
