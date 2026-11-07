---
title: 健全性v2 G2 survey — ADR health-review-integration の前提調査
tasks: [01M6VBSKZQG2YWF7NWYH6GP9P7]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 survey — ADR health-review-integration の前提調査

完了日: 2026-11-07

WorkUnit `survey`（G2 の工程「既存の prompt・review・plan・repo-health の入口を調べる」）。コードは変えていない。基準は `$CELERIS_WU_BASE`。行番号はこの WorkUnit の開始時の tree のもの。後続の WorkUnit `adr` は、この記録を根拠にして `agent-docs/adr/2026-11-07-health-review-integration.md` を書く。

## (a) prompt の組み立て口と harness ごとの経路

事実:
- 組み立ての公開口は `crates/task-worker/src/claude_code/prompt.rs` の 3 関数で、`crates/task-worker/src/claude_code.rs:129` が再公開している。
  - `build_prompt(task, context, run_id, artifacts) -> String`（prompt.rs:65）。CoS chat なら `build_cos_chat_parts` へ分かれ、それ以外は `build_prompt_inner` の後に browser の節を足す。
  - `build_prompt_inner`（prompt.rs:120）が kind で分ける: `context.execution_planner` があれば `build_execution_plan_prompt`（prompt.rs:912）。それ以外は `TaskKind::Plan` → `build_plan_prompt` / `build_project_plan_prompt` / `build_project_replan_prompt`、`TaskKind::Review` → `build_review_prompt`（prompt.rs:1635）、`Execute | Approval` → `build_execute_prompt`（prompt.rs:609）。
  - `build_prompt_with_skill_list`（prompt.rs:97）は acp・pi 用で、`build_prompt` の結果に `skills::preamble_section` を足す。
  - 共通の頭は `prompt_header`（prompt.rs:150）で、`crate::preamble::render`（`crates/task-worker/src/preamble.rs:89`）を呼ぶ。`preamble::fixed_notes`（preamble.rs:101）が production host・道具起動の禁止・run tmpdir の注意を全 run に入れる。
- harness ごとの呼び出し:
  - Claude Code: `crates/task-worker/src/claude_code.rs:517-523`。`work_dir_note` に `build_prompt` を続ける。必須規則（`CLAUDE.md`）は写さない。Claude Code が作業場所の `CLAUDE.md` を直接読むため。
  - Codex: `crates/task-worker/src/codex.rs:801-807`。`required_rules::preamble_for_run(req.cwd())` → `work_dir_note` → `build_prompt` の順。
  - Pi: `crates/task-worker/src/pi.rs:225-229`。`build_prompt_with_skill_list` の前に `rules` を足す。
  - ACP（opencode など）: `crates/task-worker/src/acp.rs:944-948`。`rules` + `build_prompt_with_skill_list`。
  - Aider: `crates/task-worker/src/aider.rs:157`。`build_prompt` だけを使い、`required_rules` を呼ばない。
- `crates/task-worker/src/required_rules.rs`: `RULES_SOURCE = "CLAUDE.md"`（:12）、`RULES_HEADING`（:15）、`preamble(cwd)`（:24）、`preamble_for_run(cwd) -> Result<String, AdapterError>`（:89）。作業場所の `CLAUDE.md` を実行時に読んで prompt の先頭に写す。git が追跡しているのに無い・空・壊れた symlink なら run を落とす。試験は `crates/task-worker/src/test_support.rs:178` が見出しの出現数を数える。
- `crates/task-worker/src/change_rules/` はまだ無い。

ADR で使う含意:
- 汎用規約（`change_rules/generic.md`）を `include_str!` で `prompt_header` か `build_prompt_inner` の共通部に 1 回だけ入れれば、Claude Code・Codex・Pi・ACP・Aider の全 harness に同じ文が届く。`build_prompt` を通らない harness は無い。ただし CoS chat run は `build_cos_chat_parts` に分かれて `prompt_header` を通らないので、入れるかどうかを決める必要がある。
- repo 固有の規則（`CLAUDE.md`）の配送経路（`required_rules`）は harness ごとの adapter にある。汎用規約は binary に埋め込み、`CLAUDE.md` は作業場所から読む。2 つは別の経路で、互いに写さない。
- Aider は `CLAUDE.md` の写しを受け取っていない（既存の穴）。ADR の範囲外として記録するか、撤去・修正の対象に入れるかを決める。

## (b) ReviewRequest・ReviewOutput と reviewer の verdict

事実:
- `crates/task-worker/src/protocol.rs`:
  - `PROTOCOL_VERSION = 4`（:32）。欄を足すときは追加だけで、版は上げない慣例（:30-31 の注記）。
  - `ReviewRequest`（:45）の欄: `summary`・`evidence: Vec<Evidence>`・`criteria: Vec<usize>`・`decisions: Vec<ReviewDecision>`・`answers: Vec<Answer>`・`checks: Vec<ReviewCheckResult>`・`policy_violations: Vec<ReviewPolicyViolation>`。後の 4 欄は `serde(default, skip_serializing_if = "Vec::is_empty")`。
  - `RunContext.review: Option<ReviewRequest>`（:433）、`RunContext.prior_review: Vec<PriorReview>`（:426）。
  - `ReviewOutput { verdicts: Vec<ReviewVerdictOut> }`（:1018）。`deny_unknown_fields` は付いていない。
  - `ReviewVerdictOut { criterion, pass, reason, repair: Option<ReviewRepairHint> }`（:1007）。
  - `ReviewRepairHint { scope, class, hint }`（:997）。`scope = "local"` だけが修復の対象になる。
- dispatcher 側の型は `crates/task-dispatch/src/review.rs:33` の `Verdict { criterion_idx, pass, reason, repair_hint }`。`review.json` の読み込みは review.rs:1269（`serde_json::from_str::<ReviewOutput>`）、名前は `REVIEW_FILE_NAME`（review.rs:175）。
- `ReviewRequest` を組むのは `run_reviewer_inner`（review.rs:1044、組み立ては :1097-1105）。値は `ReviewExtras`（`review_spawn.rs:1040-1047` で詰める）から来る。
- reviewer prompt の Result 節（prompt.rs:1785-1791）は `{"verdicts":[{"criterion","pass","reason"}]}` の形だけを指示している。`repair` 欄は prompt に書かれていない。
- worker protocol の schema は `docs/protocol/worker-protocol.schema.json`（`ProtocolSchema`、protocol.rs:1315、`review_output` は :1319）。

ADR で使う含意:
- `ReviewRequest.health: Option<…>` と `ReviewOutput.health: Option<…>` は既存の慣例どおり serde default の追加欄で足せる。`PROTOCOL_VERSION` は上げない。`docs/protocol/worker-protocol.schema.json` を再生成する。
- `ReviewOutput` に `deny_unknown_fields` が無いので、今の版の dispatcher は未知の `health` 欄を黙って捨てる。構造化 verdict を必須にするなら、型に足すのと同時に、無い・壊れているときの扱い（fail にするか advisory にするか）を決める。
- 既存の写像は `ReviewVerdictOut.repair`（局所修復のヒント）の 1 本だけ。health verdict の blocking finding は criterion の verdict に写すか、`classify_review_failure` に渡す `FailedCheck` に写すかを決める（(d) を参照）。

## (c) execution plan /3 の型と検証の入口

事実:
- `crates/task-core/src/execution_plan.rs`: `EXECUTION_PLAN_SCHEMA_V3 = "celeris.execution-plan/3"`（:23）。
  - `ExecutionPlanSpec`（:202、`deny_unknown_fields`）の欄: `schema`・`rationale`・`phases`・`work_units`・`children`・/3 用の `stages: Vec<StageSpec>`・`units: Vec<PlanUnitSpec>`・`decisions: Vec<DecisionSpec>`。/3 の欄は `skip_serializing_if = "Vec::is_empty"` で、/1・/2 の JSON を変えない。
  - `PlanUnitSpec`（:297、`deny_unknown_fields`）の欄: `requirements`・`expected_write_paths: Option<Vec<String>>`・`key`・`stage`・`kind`・`title`・`objective`・`depends_on`・`needs_decisions`・`decisions`、task だけの `acceptance`・`genre`・`skills`・`repos`・`adopt`・`gate`、leaf だけの `done_when`・`checks`・`context: UnitContext`・`harness`・`budget`・`outputs`、両方の `features`。
  - `internal_view`（:401）が /3 を /2 の形に写す。schema は `docs/protocol/execution-plan.schema.json`（`schema_value`、:506。`UPDATE_SCHEMA=1` で再生成）。
- 検証は `crates/task-core/src/execution_plan/validation.rs`: `validate`（:925）→ `validate_with(spec, limits, done_work_units, PlanContext)`（:953）→ /3 なら `validate_v3`（:1408）。エラーは `PlanValidationError`（:92）、上限は `ExecutionLimits`（:13）。
- `validate_with` の呼び出し元: planner の計画は `crates/task-dispatch/src/dispatcher/planner_flow.rs:158`（上限を緩めた再検証が :173）、人の計画は `crates/task-ops/src/execution.rs:281`・`:354`、採用時の再検証は `adopt_plan_with_children`（execution.rs:94）、replan は `replan`（execution.rs:501、検証は :627）。ほかに `crates/task-ops/src/tree_plan.rs:98` と `crates/task-ops/src/replay.rs:171`。
- planner の prompt で /3 の書き方と check の規則を出すのは `build_execution_plan_prompt`（prompt.rs:912）と `PLANNER_CHECK_GUIDANCE`（prompt.rs:1797）。

ADR で使う含意:
- 設計 brief は `ExecutionPlanSpec` の /3 専用の欄（例 `design_brief: Option<DesignBrief>`、`skip_serializing_if`）として足すのが既存の形に合う。unit ごとに持たせるなら `PlanUnitSpec` に足す。どちらも `deny_unknown_fields` なので、型に足さない限り planner は書けない。
- brief の形の検証は `validate_v3` の中に置けば、planner・人の計画・採用・replan・replay の全経路が同じ検査を通る。`validate_with` の呼び出し元を個別に変えなくてよい。
- risk の判定に使う write paths は既にある `PlanUnitSpec.expected_write_paths` と leaf の `context`（`UnitContext`）から取れる。
- `docs/protocol/execution-plan.schema.json` の再生成と `PLANNER_CHECK_GUIDANCE` か `build_execution_plan_prompt` への 1 節の追加が必要になる。

## (d) dispatcher: reviewer run・plan 採用・review 失敗の写像・candidate SHA

事実:
- reviewer run を起こす所: `crates/task-dispatch/src/dispatcher/review_spawn.rs` の `spawn_review`（:345）。
  - reviewer の選択は `pick_reviewer`（:434 で呼び、定義は :1381）。
  - review 前の target 同期（ADR-0118）は :486 から。条件は `pre_review_sync_enabled()`（:183）・repo がある・remote でないこと。結果は repo ごとの `ReviewSync { repo_id, name, dir, target_ref, target_sha, before_sha, reviewed_sha }`（:126）。
  - 合格の再利用は `reviewer_reuse`（:715 で呼び、定義は :1155）。`bound_reviewer_pass`（:66）が同期前の HEAD に束ねた合格を探し、target 側の変更と候補の差分が重ならなければ `Event::ReviewerPassReused` を残して reviewer run を省く。
  - 検査と reviewer は `review_task`（:1049、定義は `crates/task-dispatch/src/review.rs:620`）。reviewer run は `run_reviewer`（review.rs:982）と `run_reviewer_inner`（review.rs:1044）。
- candidate / merge candidate SHA の記録（ADR-0118 D3）: review_spawn.rs:774-777 で `DeliveryRow` の `target_sha`・`reviewed_sha`・`merge_candidate_sha` を更新する（欄は `crates/task-core/src/delivery.rs:107`）。review_spawn.rs:787-798 で `Event::ReviewTargetSynced { review_run, repo_id, target_ref, target_sha, before_sha, reviewed_sha, merge_candidate_sha, attempt }`（`crates/task-core/src/model/events.rs:151`）を追記する。
- 候補に束ねた結果の再利用の前例は 2 つある。
  - `Event::RequiredChecksEvaluated { review_run, repo_id, target_sha, reviewed_sha, definition_sha256, results }`（events.rs:164）。書くのは `crates/task-dispatch/src/dispatcher/review_verdict.rs:215`、再利用の判定は `crates/task-core/src/required_checks.rs` の `reusable_passes`（:48）と `definition_digest`（:10）。
  - `Event::ReviewerPassed`（events.rs:184）と `Event::ReviewerPassReused`。
- plan の採用: `planner_flow.rs`。
  - 検証（:158）→ `tree_plan_gate`（:243、定義は `crates/task-dispatch/src/dispatcher/tree_units.rs:11`）。
  - 初回は `task_ops::execution::adopt_plan_with_children`（planner_flow.rs:354、定義は `crates/task-ops/src/execution.rs:76`）、replan は `task_ops::execution::replan`（planner_flow.rs:342）。
  - 採用の後に `root_plan_approval`（planner_flow.rs:375、定義は tree_units.rs:331）が `task_core::tree::plan_approval`（`crates/task-core/src/tree/approval.rs`、`PlanApprovalFacts`:13・`PlanApproval`:33）で人の承認を要るか決める。要れば `Trigger::PlanGate` と `Event::PlanApprovalRequested { plan_id, reasons }`（planner_flow.rs:392）。reasons は機械が読める文字列（`decisions:…`・`review_human:<stage>`・`near_limit:…`）。
- review 失敗の写像: `review_verdict.rs` の `on_review_finished`（:15）。
  - 不合格で `TaskKind::Execute` なら `try_review_repair`（:495 で呼び、定義は :624）→ `task_core::classify_review_failure`（`crates/task-core/src/execution.rs:878`）。返り値は `RepairDecision::Repairable(class)` か `Substantive`。repairable なら repair WU（`apply_review_repair`、:830）。
  - それ以外は `Trigger::ReviewFail`（:553）。WU のある task の再 dispatch では `replan_gate`（`crates/task-dispatch/src/dispatcher/work_units.rs:1540`）が `RunPlanner { replan: true }` を返す。replan の理由は `replan_trigger_reason`（planner_flow.rs:970）が直近の `ReviewVerdict{pass:false}` から作る。
  - `review_rework` は dispatcher に無い。CoS の判断を API が検査する: `crates/task-api/src/cos/basis/review_rework.rs`（`REVIEW_REWORK_MAX = 2`、:24。verdict の理由の形 `reviewer(<id>): <text>` を `reviewer_verdict`（:36）が分ける）。

ADR で使う含意:
- 設計 review を起こす場所は、採用の直後で unit を起こす前（`root_plan_approval` と同じ位置）が既存の分岐に合う。`PlanApproval.reasons` に `design_review:<risk>` を足す形か、reviewer run を起こす別の状態にするかを選ぶ。今の `PlanGate` は人の承認だけを待つ。
- health の材料（diff 統計・owner 地図行・G1 の new findings）は `spawn_review` が `ReviewSync` を作った後に作れば、`target_sha`・`reviewed_sha` が決まっている。`RequiredChecksEvaluated` と同じく `(repo_id, target_sha, reviewed_sha, 定義 digest)` に束ねた event に残せば、再 review と `reviewer_reuse` は同じ候補で作り直さない。WU ごと・tick ごとには作らない。
- health verdict の blocking finding を repair/replan に写すには、既存の `classify_review_failure` の入力（`FailedCheck`）に載せるのが最小の変更になる。種別ごとに局所修復（`ReviewerLocal`）と substantive（→ `ReviewFail` → replan）に振り分ける表を ADR に置く。`review_rework` の検査は verdict の理由の文字列（`reviewer(<id>): …`）を見るので、health finding の理由もこの形に乗せれば CoS 側を変えずに済む。

## (e) scripts/dev/repo-health/ の CLI・profile・report

事実:
- CLI は `scripts/dev/repo-health/repo_health.py`（`parse_args`、:39）。引数は `--repo`（必須）・`--print-base`・`--base <rev|auto>`・`--head <commit|worktree>`・`--policy-version`・`--profile <name|path>`・`--rules`・`--out`。exit は 0 合格・1 必須 rule の fail・2 error/unknown/unsupported（:13）。検証器は `scripts/dev/repo-health/validate_report.py`。
- 単一入口は `scripts/dev/check-health.sh`（`--base auto --head worktree --policy-version 1`）。`scripts/dev/test-affected.sh:44-45` が `--list` 以外で必ず実行する。
- profile の解決と読み込みは `scripts/dev/repo-health/health/profile.py`。
  - 解決順は `--profile` > repo の `profiles/default.toml` > 組み込み `BUILTIN_GENERIC`（:53）。
  - policy は base 側で読む（:7、base → bootstrap → builtin、:263-275）。
  - 最上位の項目は `_TOP_KEYS = {version, extends, description, policy_paths, exclude_paths, doc_only, adapters, classes, arch_map, metrics, rules}`（:61）。未知の項目は読み込みエラー（:160-162）。rule ID は `_RULE_ID`（:63）で固定。
- `scripts/dev/repo-health/profiles/default.toml`（8 行）は `extends = "celeris"` だけ。
- `scripts/dev/repo-health/profiles/celeris.toml` の節（行は開始位置）: 最上位の `version`・`policy_paths`（:11）・`exclude_paths`（:23）、`[doc_only]`（:25）、`[adapters]`（:29）、`[metrics.*]`（:34・:39・:46）、`[arch_map]`（:53、`path = "docs/architecture-map.md"`・`reference = capability-map.json`）、`[rules."arch-map.*"]`（:59-75）、`[[rules."deps.violation".layers]]`（:81-109、core/services/runtime/control/edge/bin の層）、`[rules."size.responsibility-growth"]`（:114）、例外（:123・:127）、既存検査の包み `doc.links`・`adr.numbers`・`progress.index`・`human-text`・`source-size`（:135-163）。`[review]` 節は無い。
- report の schema は `scripts/dev/repo-health/schema/report-v1.schema.json`（`celeris.repo-health.report/1`）。
  - 最上位: `schema`・`tool`・`inputs`・`snapshot`・`policy`・`rules`・`findings`・`summary`・`metrics`。
  - finding の欄: `rule_id`・`fingerprint`（`<rule_id>:<hash>`、行番号を含まない）・`severity`（error|warning|info）・`delta`（new|worsened|existing）・`candidate`（静的推定は true）・`evidence`・`snapshot`・`scope`・`exception`・`rationale`。
  - `summary` の欄: `verdict`（pass|fail|error）・`exit_code`・`reviewable`（head が commit で dirty でなく base に届くときだけ true）・`counts {findings, fail_grade, new, worsened, existing}`。
  - `policy` の欄: `version`・`source`（base|bootstrap|builtin|unavailable）・`changed`・`files[{path, base_blob, head_blob}]`。
- 例は `schema/examples/`、壊れた例は `schema/bad-examples/`。

ADR で使う含意:
- `[review]` 節を足すには `_TOP_KEYS` に `review` を足し、`profile.py` の docstring（項目の正本）に書く。未知の項目がエラーになるので、検査器より先に profile へ書くことはできない。
- dispatcher が「base 側の版」を読む規則は、profile.py が既に持つ「policy は base 側で読む」と同じ考え方になる。dispatcher が自分で TOML を読むなら `git show <target_sha>:<path>` で読む形にそろえる。
- 「new findings の要約」は `findings[]` のうち `delta ∈ {new, worsened}` で `exception == null` のものと `summary.counts` を使う。`reviewable == false` の report は review の証跡にしない（schema の注記）。G1 の report は `--head <commit>` で `reviewed_sha` を渡せば候補 SHA に束ねられる。
- `celeris.toml` は `policy_paths` に `^scripts/dev/repo-health/` を含む。`[review]` 節の追加そのものが policy.changed になる（(f) を参照）。

## (f) G1 進捗の「提案」にある policy file 変更 branch の扱い

事実:
- `agent-docs/progress/2026-11-07-health-v2-g1.md:50`: 「policy の file を変える branch の通し方（policy だけの別 review の後に main を base にして再評価）を G2 で決める。それまで、main に profile が入った後の検査器の変更は check-health が落ちる。」
- 同 :48 は `docs/guides/source-health.md` の更新を提案している（G1 の write_set の外だったため未実施）。
- 契約は `agent-docs/adr/2026-11-07-repo-health-contract.md:13`・`:39-40`。policy は base 側で評価する。head が policy の file を変えれば `policy.changed`（必須、severity error、delta new）で fail（exit 1）になる。承認はその変更だけを対象にした別の review（人か、G2 で reviewer に渡す policy review）で行い、承認後の main を base にして再評価する。base に profile が無い初回は bootstrap（warning）。
- `agent-docs/adr/2026-11-07-repo-health-celeris-profile.md:26` と `scripts/dev/repo-health/rules/policy_changed.py:1-3`・`:24` も同じ規則を書いている。

ADR で使う含意:
- 今のままでは、検査器・profile を変える G2 以降の実装 branch は全部 `test-affected.sh` → `check-health.sh` で exit 1 になる。G2 は「policy 変更を別の review 対象にする」具体的な経路を決める必要がある。候補は 3 つ: policy の file だけを変える独立 task/branch にして人か policy review で承認する、`ReviewRequest.health` に policy.changed を載せて reviewer の policy verdict を必須にする、WU の check では policy.changed を advisory にして final review でだけ必須にする。
- どの経路でも「候補が policy を弱めて自分を通す」ことを防ぐため、評価は base（target）側の policy で行う。この前提は契約の D2 のまま変えない。

## (g) CLAUDE.md と docs/guides/source-health.md のうち汎用規約と重なる文

事実:
- `CLAUDE.md` の「変更の方針（破壊的変更を是とする）」節（:25-31）。旧い仕組みは同じ変更で消す・deprecated/互換分岐/警告だけ/新旧切替 flag を残さない（:27）、「念のため」残さない（:28）、review では旧い仕組みの残りを不備として指摘する（:31）。
- `CLAUDE.md` の「禁止」節（:33-）。`unwrap()` の禁止、タスクの状態を会話やワーカーのメモリに持たせない（真実は SQLite、:36）、自己申告だけで完了にしない。
- `CLAUDE.md` の「作業の進め方」節にある設計判断 → ADR → 実装の順、同じアプローチを 3 回失敗したら人に聞く、CPU 焼き負荷の禁止と決定的再現。
- `docs/guides/source-health.md`（51 行）:
  - :7-12 は `test-affected.sh` が `check-health.sh` を必ず実行すること。
  - :14-16 は軽量検査の範囲。
  - :18-36 は source-size の固定基準・許容値・独立レビュー必須。:34-35 は「SHA・分類・閾値を変える際は、差分、理由、負債の追跡先を明記した独立レビューを必須とする」。
  - :38-51 は証拠と追跡起票。
- `source-health.md` の front matter は 1 行目の題名の後（:3-5）にある。:45 の link は `../progress/2026-10-23-source-health.md` で、実在するのは `docs/progress/2026-10-23-source-health.md`（`agent-docs/progress/` には無い）。

ADR で使う含意:
- 汎用規約の finding 種別のうち「新旧切替・未承認 fallback」と「旧経路の撤去漏れ」は `CLAUDE.md:27-28,31` と同じ内容になる。「状態の二重正本」は `CLAUDE.md:36` と近い。`change_rules/generic.md` は repo に依らない一般形で書き、Celeris 固有の言い回し（SQLite・events 追記専用など）は `CLAUDE.md` を参照して写さない。
- `source-health.md:34-35` の「基準・閾値の変更は独立レビュー」は (f) の policy review と同じ規則になる。G2 の ADR でこの文を policy review の決定への参照に置き換えるか、guide 側で ADR を参照する。
- `source-health.md` は入口・profile・report の説明が G1 の後の実態（`check-health.sh` が既存検査を rule として包む）に合っていない（G1 提案 :48）。

## (h) 撤去候補になる旧い仕組み

事実（候補。撤去するかは ADR で決める）:
1. reviewer prompt の中の規約文: `build_review_prompt`（prompt.rs:1635）の「Commit references in progress files and reports」節（:1645-1655）は `CLAUDE.md` の進捗ファイルの hash 規則と同じ内容を英語で写している。同じ関数の policy violation の節（:1718-1735）は `ReviewRequest.policy_violations` の構造化された材料を文で説明している。
2. planner prompt の規約文: `PLANNER_CHECK_GUIDANCE`（prompt.rs:1797-1870）は `CLAUDE.md` の規則（ADR-0128 の記録場所・全体試験は release gate だけ・`test-affected.sh`/clippy/fmt・CPU 焼き負荷の禁止・hash を書かない）を英語で写している。`PRODUCTION_HOST_PLANNER_GUIDANCE`（:1874）と `preamble::fixed_notes`（preamble.rs:101）の `production_host_note` も同じ規則を 2 か所に持つ。
3. 健全性の説明の重複: `docs/guides/source-health.md` と `agent-docs/adr/2026-11-07-repo-health-contract.md`・`2026-11-07-repo-health-celeris-profile.md`・`scripts/dev/repo-health/health/profile.py` の docstring・`scripts/dev/check-health.sh` の冒頭注記が、入口・base・policy の扱いを別々の文で書いている。
4. 未記載の欄: reviewer prompt の Result 節（prompt.rs:1785-1791）は `ReviewVerdictOut.repair` を書かせていない。構造化 health verdict を足すときに同じ節を作り直すことになる。
5. 配送の穴: Aider（aider.rs:157）だけが `required_rules` の前置きを受け取らない。

ADR で使う含意:
- 撤去の対象として確実に挙げられるのは 1 と 3。汎用規約の正本（`change_rules/generic.md`）と重なる reviewer prompt の文は同じ変更で消し、generic.md か `CLAUDE.md` の参照に置き換える。
- 2 は planner 向けの「check の書き方」で、多くは Celeris の検査機構（`CELERIS_WU_*`・scope check）に固有の手順になる。汎用規約とは重ならない部分が多い。撤去は `CLAUDE.md` と同じ文（記録場所・試験範囲・負荷）に限る候補として書き、残りは残す理由を ADR に書く。
- 3 は `source-health.md` を ADR・`check-health.sh` への参照に縮める形が `CLAUDE.md:27` の方針に合う。`source-health.md` の誤った link と front matter の位置も同じ変更で直す候補になる。

## 検査（提出時の HEAD で実行）

- 受け入れ条件 0: (a)〜(h) の各節に実在する path を挙げた（下の `test -e` で確かめた）。
- 受け入れ条件 1: `git diff --quiet "$CELERIS_WU_BASE" -- crates/ scripts/`（exit 0）。

## 未解決事項

- CoS chat run（`build_cos_chat_parts`）に汎用規約を入れるかは未決（(a)）。
- Aider への `CLAUDE.md` の配送漏れを G2 で扱うかは未決（(a)・(h) 5）。

## 提案

- ADR では、`ReviewOutput.health` が無い・壊れているときの扱いと、`classify_review_failure` への写像表（finding 種別 → `ReviewerLocal` か substantive）を同じ節に置く。
