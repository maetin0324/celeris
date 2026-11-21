---
title: 戦略層 v1 子 2 request-prompt — Project.request の全文と sha256 を worker・planner・reviewer・WU の実 prompt に出す（U3）
tasks: [01M7XQ9Q9EB3CH59TSNYM9HBC8]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# request-prompt（WorkUnit。子 2 の段 impl）

完了日: 2026-11-21。設計は [ADR 2026-11-21-strategy-evidence-contract](../../adr/2026-11-21-strategy-evidence-contract.md) D5。
付記は [ADR 2026-11-21-ancestor-objective-in-prompts](../../adr/2026-11-21-ancestor-objective-in-prompts.md) の「付記 U3」。

## 結論

- 案件に属する task の worker・planner・WU・final review の reviewer（と設計 review）の prompt に、`Project.request` の全文（切り詰めない）と
  sha256 の節「案件の依頼（Project.request の全文。人の元の目的の正本。sha256: …）」がちょうど 1 回載る。案件に属さない task の prompt は変わらない。
- 子 1 の prompt-audit の欠け 5（15 run のすべてで Project.request の末尾が prompt に無い）は、この変更を含む release の昇格後の再監査で
  確かめる（未実施。D5.7）。
- 開始時の確認（2026-11-21T15:26Z、`curl -s http://127.0.0.1:7710/api/v1/health`）: 本番の health の release は main の commit
  「cluster-wait decisions: 進捗と ADR の名前を実装に合わせる」を HEAD とする release で、子 1 の統合 commit「integrate wu/gate (phase close)」を
  祖先に持つ（`git merge-base --is-ancestor`、exit 0）。

## 変更（commit を件名で挙げる）

- 「strategy-v1 2 request-prompt: Project.request の全文と sha256 を worker・planner・reviewer・WU の prompt に出す（U3）」
  - `crates/task-worker/src/protocol.rs`: `TaskLineageContext.project: Option<ProjectRequestContext>`（`{project_id, request, request_sha256}`）。
    `None` のとき wire に出ない。`is_empty` は `project` も見る。`RunContext` の欄の数は変えていない。`PROTOCOL_VERSION` は 5 のまま（欄の追加だけ）。
  - `crates/task-worker/src/claude_code/prompt.rs`: `prompt_header` の 2 か所（WU と WU でない run）が `lineage_section`
    （`project_request_section` → `ancestors_section`）を出す。harness ごとの分岐は無い。
  - `crates/task-dispatch/src/prompt_ancestors.rs`: `project_request`（`chain_to_root` の列で最初に `project_id` を持つ task の案件）と
    `chain_lineage`。worker・planner・WU（`lineage`）、final review（`review_human_inputs`）、設計 review（`design_gate` が `lineage` を使う）が同じ関数を通る。
  - `crates/task-dispatch/src/review.rs`・`dispatcher/review_spawn.rs`: `ReviewHumanInputs.ancestors` と `ReviewHumanInputs::lineage()` を
    `ReviewHumanInputs.lineage`（`chain_lineage` が組む）に置き換えた（旧い欄と関数は消した）。
  - `docs/protocol/worker-protocol.md`・`worker-protocol.schema.json`: `context.lineage.project` を書いた。
- 「strategy-v1 2 request-prompt: project_wake の手元の sha256_hex を消し task_core::project_proposal::sha256_hex を使う」
  - `project_wake::project_request_sha256` と prompt の `request_sha256` は同じ `task_core::project_proposal::sha256_hex` で計算する。
    手元の `sha256_hex(&[u8])` は消した（snapshot と root の idempotency の hash も同じ関数。入力は UTF-8 の文字列で値は変わらない）。
- 「strategy-v1 2 request-prompt: 行の無い案件を付けていた review の試験 2 件の fixture に案件の行を置く」
  - D5.3（案件を持つのに行を読めなければ run を起こさない）により、`ProjectId::new()` だけを task に付けた試験 2 件が tick の Err で落ちた
    （下の「提出時の検査」の 1 回目）。API は行の無い案件を task に付けられないので、fixture が実際に起きない状態を置いていた。
    試験が守る不変条件（review のやり直しの後に done の報告が 1 件、承認の子が親の project・milestone・assignee を継ぐ）は変えていない。
- 「strategy-v1 2 request-prompt: 進捗を書く」（この file）。

高 risk の path（task-worker の db_guard・subprocess・tool_policy、task-dispatch の worker_finish・leases・run_supervisor・run_settlement、
review-gate の `health_review/(design|design_gate|…).rs` など）は変えていない。`sh "$CELERIS_WU_SCOPE_PATHS"` の 17 path（と fixture の
`dispatcher/tests/review.rs`、この file）を高 risk の正規表現に当てて一致 0 件（grep exit 1）。`health_review/design/tests.rs` は試験の file で、
正規表現 `health_review/design\.rs$` に当たらない。

## 試験（prefix `prompt_project_request_`）

| crate | 試験 | 守る不変条件 |
|---|---|---|
| task-dispatch | `dispatcher::tests::prompt_project_request::prompt_project_request_worker_and_reviewer_runs_carry_the_request` | 3,000 字超の request の末尾の印・見出し・sha256 が worker と reviewer の prompt に 1 回ずつ |
| task-dispatch | `…::prompt_project_request_planner_and_work_unit_runs_carry_the_ancestor_project` | 案件を root だけが持つ木で、planner と WU の prompt に 1 回ずつ |
| task-dispatch | `…::prompt_project_request_task_without_project_has_no_section` | 案件の無い task の prompt に節が無い |
| task-worker | `claude_code::tests::project_request_prompt_tests::prompt_project_request_every_builder_carries_the_full_request_once` | 全 builder・2 harness で節が 1 回 |
| task-worker | `…::prompt_project_request_task_without_project_is_unchanged` | 案件の無い context では、節を除くと prompt が 1 byte も違わない |
| task-worker | `…::prompt_project_request_lineage_with_only_a_project_is_on_the_wire` | `lineage` が案件だけでも wire に出て往復する |

既存の試験で検出できない理由: 子 1 の `prompt_ancestor_objective_*` は祖先の task の文だけを見て、`Project.request` を store に置かない。

### 変更後（提出時の HEAD）

- `cargo test -p task-dispatch prompt_project_request` → `test result: ok. 3 passed; 0 failed`。
- `cargo test -p task-worker prompt_project_request` → `test result: ok. 3 passed; 0 failed`。
- 回帰: `cargo test -p task-dispatch prompt_ancestor_objective` → 3 passed、`cargo test -p task-worker prompt_ancestor_objective` → 2 passed、
  `cargo test -p task-dispatch project_wake` → 200 passed、`cargo test -p task-dispatch project_controller` → 18 passed、
  `cargo test -p task-dispatch dispatcher::tests::review::` → 35 passed。

### production の変更だけを戻した tree（変更前に落ちること）

手順: `git worktree add --detach "$TMPDIR/revert-u3" <base>`（base は `$CELERIS_WU_BASE` と同じ tree の commit「integrate wu/contract-design (phase design)」）に、
試験の file だけ（`dispatcher/tests/prompt_project_request.rs`・`dispatcher/tests/mod.rs`・`dispatcher/tests/prompt_ancestor_objective.rs`
（helper の可視性だけ）・`claude_code/project_request_prompt_tests.rs`・`claude_code/tests.rs`・`claude_code/ancestor_prompt_tests.rs`）を写した。

- `cd "$TMPDIR/revert-u3" && cargo test -p task-dispatch prompt_project_request` → exit 101、`test result: FAILED. 1 passed; 2 failed`。
  落ちた 2 件は 4 経路すべてで 3 つの印が x0（want x1）:
  `worker`・`reviewer`・`planner`・`work unit` × `project heading`・`project request tail`・`project sha256`（12 行）。
  通った 1 件は `prompt_project_request_task_without_project_has_no_section`（変更前から節が無いので通るのが正しい）。
- `cd "$TMPDIR/revert-u3" && cargo test -p task-worker prompt_project_request` → exit 101、compile できない:
  `error[E0432]: unresolved import crate::protocol::ProjectRequestContext` 1 件、`error[E0560]: struct TaskLineageContext has no field named project` 3 件。
- 注意: 戻した tree を残したまま同じ `CARGO_TARGET_DIR` で本物の tree の試験を走らせると、cargo は戻した tree の build を再利用し、
  本物の tree でも 2 件が落ちた。`git worktree remove` の後は本物の tree が再 build され、3 件とも通った。

### e2e の protocol の試験（test-affected は e2e を選ばない）

- `cargo build -p celeris && cargo nextest run -p e2e --no-tests=fail -E 'binary(scenarios) | binary(delegation_scenarios)'` →
  `9 tests run: 9 passed, 0 skipped`。`"protocol":5` の literal は変わらない（版を上げていない）。

## 提出時の検査（提出時の HEAD で実行）

- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor の deno_core の deprecated の warning 1 件だけ。workspace の lint は 0）。
- `bash scripts/dev/test-affected.sh`（対象: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）:
  - 1 回目（fixture を直す前）: exit 100、`6179 tests run: 6177 passed, 2 failed, 28 skipped`。落ちた 2 件は上の fixture の 2 件
    （`Store(Invalid("project … of task … is not in the store; the run's Project.request cannot be read"))`）。
  - 2 回目（fixture を直した後）: 下の「未解決」を見よ。

## LoC（`git diff --numstat` の base から HEAD。file の分類は path で行った）

- production の Rust: +100 −37（純増 63）。内訳（純増）は protocol.rs +24、prompt.rs +21、prompt_ancestors.rs +35、review.rs −10、
  review_spawn.rs +1、project_wake.rs −8（手元の hash 関数を消した分を含む）。純増の理由: 新しい欄 1 つ（`TaskLineageContext.project`）と、それを読む関数・出す関数。
  新しい表・migration・event の種類・状態機械・trait・Manager は無い。
- 試験: +346 −13（純増 333）。文書（ADR の付記・protocol の md と schema・この file）は別。

## 親の目的への寄与

- 確認済み（試験）: 人の元の目的の正本（Project.request）の全文が、CoS の写しを経ずに worker・planner・WU・reviewer に届く。
  子 3 以降の戦略の文書が前提にする `request_sha256` と同じ値を run も見る（改善 B の出典の追跡、observability の土台）。
- 未確認: 本番の実 prompt で欠け 5 が 0 になること（release の昇格は人が行う。D5.7 の再監査は昇格の後）。

## 未解決

- D5.7 の再監査（昇格後に子 1 の prompt-audit の collect と render）は未実施。
- test-affected の 2 回目の結果は result.json と報告に書く（この file を commit した後に走らせる）。

## 提案

- D5.3 の fail-closed は、行の無い案件の task が 1 件でもあると dispatcher の tick ごと Err になる（試験 2 件がそれを示した）。本番に
  該当は 0 件で、行を消す経路も無いが、tick 全体でなく、その run だけを起こさない形（例: その task を blocked にして人に知らせる）に
  狭めるかを、子 2 の close か後続で決める。
