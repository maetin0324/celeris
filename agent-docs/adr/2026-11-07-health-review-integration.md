---
tasks: [01M6VBSKZQG2YWF7NWYH6GP9P7]
---
# ADR 2026-11-07: 健全性を設計と review に組み込む（規約配布・設計 brief・risk 判定・構造化 health verdict）

- 日付: 2026-11-07
- 状態: 採用（設計）。この ADR はコードを変えない。実装は健全性 v2 G2 の後続 unit（D8 の U1〜U6）。
- 根拠: 前提調査 [G2 survey](../progress/2026-11-07-health-v2-g2/adr-survey.md) の (a)〜(h)。行番号は survey の時点のもの。
- 関係: [ADR 2026-11-07 汎用 repo health の契約](2026-11-07-repo-health-contract.md)（D2: policy は base 側で評価、policy.changed）、[ADR 2026-11-07 repo health の Celeris profile](2026-11-07-repo-health-celeris-profile.md)、[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md)、[ADR 2026-10-23 必須規則の prompt 配送](2026-10-23-required-rules-prompt-delivery.md)、[2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)（review 前の target 同期と candidate SHA）。

## 結論
- 変更の仕方の汎用規約は `crates/task-worker/src/change_rules/generic.md` の 1 file を正本にし、`include_str!` で `claude_code::prompt` の `prompt_header` から全 harness の prompt へ 1 回だけ入れる。repo 固有の規則は今までどおり `CLAUDE.md`（`required_rules`）で配る。2 つは互いに写さない。
- repo 固有の review 方針は `scripts/dev/repo-health/profiles/<profile>.toml` の `[review]` 節に置き、dispatcher は `repo_health.py --print-review-policy` で **base（target）側**の版を読む。
- planner の /3 計画に設計 brief（`ExecutionPlanSpec.design_brief`）を持たせ、risk を決定的に判定し、高 risk の計画だけ採用の前に既存の reviewer run で設計 review する。
- 実装後の review には `ReviewRequest.health` を candidate SHA ごとに 1 回だけ作って渡し、reviewer は `ReviewOutput.health` に固定 7 種の finding を返す。blocking の finding は既存の `classify_review_failure`（局所修復）か `Trigger::ReviewFail`（replan）に写す。
- policy の file を変える変更は policy の file だけの branch に分け、reviewer の policy verdict と人の承認で通す。

## 根本原因
- 健全性 v2 の B0・G1 で、変更が「どこで直すか・何を消すか」を決めないまま実装される事例を数えた（[健全性 v2 の基準](2026-11-07-health-v2-baseline.md)）。今の流れには設計を確かめる段が無い。planner の /3 計画は unit と check を書くだけで、原因・owner・撤去対象を書く欄が無い（survey (c)。`ExecutionPlanSpec`・`PlanUnitSpec` は `deny_unknown_fields`）。
- reviewer は受け入れ条件ごとの pass/fail（`ReviewVerdictOut`）しか返さない（survey (b)）。「owner の外で症状を直した」「旧経路を消していない」は自由文の reason に埋もれ、dispatcher は repair と replan を分けられない。
- reviewer は G1 の repo health の結果（new findings・policy.changed）を受け取らない。`ReviewRequest` に health の材料が無い（survey (b)・(e)）。
- 変更の規約は `CLAUDE.md`・`docs/guides/source-health.md`・reviewer prompt・planner prompt に別々の文で写されている（survey (g)・(h)）。どれが正本か分からず、直すと食い違う。
- G1 の契約で、policy の file を変える branch は policy.changed で必ず落ちる（survey (f)）。通す経路が決まっていないので、検査器・profile を変える G2 以降の branch は `test-affected.sh` を通れない。

## 責務 owner
| 責務 | owner（crate/module・file） | 理由 |
|---|---|---|
| 汎用規約の文 | `crates/task-worker/src/change_rules/generic.md` | binary に埋め込み、repo に依らない |
| 規約を prompt に入れる | `crates/task-worker/src/claude_code/prompt.rs` の `prompt_header` | 全 harness の `build_prompt` が通る唯一の組み立て口（survey (a)） |
| repo 固有の必須規則 | `CLAUDE.md` と `crates/task-worker/src/required_rules.rs` | 既存（ADR 2026-10-23） |
| repo 固有の review 方針の文 | `scripts/dev/repo-health/profiles/<profile>.toml` の `[review]` | profile が repo 固有の値の唯一の置き場（G1 D3） |
| profile の解釈（extends・base 側の読み込み） | `scripts/dev/repo-health/health/profile.py` | profile を読む実装は 1 つだけにする |
| 設計 brief の型と形の検証 | `crates/task-core/src/execution_plan.rs`・`execution_plan/validation.rs` の `validate_v3` | 全経路（planner・人の計画・採用・replan・replay）が通る |
| risk の判定（純粋関数） | `crates/task-core/src/design_review.rs`（新規） | I/O なしで試験できる |
| 設計 review を起こす・結果で採用する | `crates/task-dispatch/src/dispatcher/planner_flow.rs` | 計画の検証と採用の流れの持ち主 |
| health の材料を作る・再利用する | `crates/task-dispatch/src/dispatcher/review_health.rs`（新規。`review_spawn.rs` から呼ぶ） | `ReviewSync` で candidate SHA が決まる場所の直後 |
| health verdict の読み込みと写像 | `crates/task-dispatch/src/review.rs`（読み込み）・`review_verdict.rs`（写像） | 既存の review.json 読み込みと repair/replan の分岐 |
| wire の型 | `crates/task-worker/src/protocol.rs` | `ReviewRequest`・`ReviewOutput` の持ち主 |

## 既存の再利用先
- prompt: `prompt_header`（prompt.rs:150）。execute・plan・replan・execution plan・review の各 builder がこれを呼ぶ。Aider も `build_prompt` を通るので汎用規約は届く。
- 計画: `ExecutionPlanSpec` の /3 専用欄の慣例（`skip_serializing_if`、/1・/2 の JSON を変えない）、`validate_v3`、`PlanValidationError`、schema の再生成（`docs/protocol/execution-plan.schema.json`、`UPDATE_SCHEMA=1`）。
- 計画の書き方の指示: `build_execution_plan_prompt`（prompt.rs:912）。
- reviewer の起動: `pick_reviewer`（review_spawn.rs:1381）・`run_reviewer_inner`（review.rs:1044）・`REVIEW_FILE_NAME`。設計 review も同じ run の形で起こす。新しい run 種別は作らない。
- 候補 SHA に束ねた再利用: `Event::RequiredChecksEvaluated` と `required_checks::reusable_passes`・`definition_digest` の形。`reviewer_reuse`（review_spawn.rs:1155）で reviewer を省くときは health も作らない。
- 失敗の写像: `FailedCheck { check: Check::Reviewer, reason, repair_hint }` → `classify_review_failure` → `RepairClass::ReviewerLocal(ReviewerRepairKind)` か `Substantive` → `Trigger::ReviewFail` → `replan_gate`。CoS の `review_rework` は verdict の reason の形 `reviewer(<id>): <text>` を読むので、health の reason も同じ形に乗せる。
- 人の承認: `Trigger::PlanGate`・`Event::PlanApprovalRequested { plan_id, reasons }`・`PlanApproval.reasons` の機械可読な文字列。
- repo health: `repo_health.py --base <rev> --head <commit>`、report の `findings[].delta`・`exception`・`summary.reviewable`・`policy.{source,changed,files}`（survey (e)）。

## 選択肢
1. 汎用規約の置き場所
   - A（採用）: task-worker に埋め込む 1 file（`change_rules/generic.md`）。どの repo の run にも同じ文が届き、作業ツリーに依らない。
   - B: 各 repo の `CLAUDE.md` に書く。repo ごとに写しが増え、Celeris 以外の repo では届かない。
   - C: KB の skill にする。配送が harness と課の設定に依り、届かない run が出る。
2. repo 固有の review 方針の置き場所
   - A（採用）: profile の `[review]` 節。base 側で読む仕組みが既にある。
   - B: `celeris.toml`（daemon の config）。repo の外にあり、変更が review を通らない。
   - C: `docs/architecture-map.md` に列を足す。地図は owner の正本だが、高 risk の path class と保護する不変条件の名前は地図の行と一対一にならない。
3. dispatcher が profile を読む方法
   - A（採用）: `repo_health.py --print-review-policy` を base の SHA で呼ぶ。profile の解釈は `profile.py` の 1 つだけ。
   - B: Rust で TOML を読む。`extends` の解決と base 側の読み込みを 2 つ目の実装として持つことになる。
4. 設計 brief の位置
   - A（採用）: 計画単位の `ExecutionPlanSpec.design_brief`。1 つの task の変更に原因と owner は 1 つ。
   - B: unit ごと（`PlanUnitSpec`）。同じ原因を unit ごとに書き、食い違う。
5. 設計 review の時点
   - A（採用）: 計画の検証の後、採用（`adopt_plan_with_children`）の前。差し戻しで unit の行を作り直さない。
   - B: 採用の後、unit の起動前（`root_plan_approval` の位置）。差し戻すと採用済みの unit を replan で消すことになる。
   - C: 全計画に人の承認。人の負担が大きい。
6. health の材料を作る時点
   - A（採用）: task の review（`spawn_review`）で `ReviewSync` の後、candidate SHA ごとに 1 回。
   - B: WU の check ごと。WU の数だけ repo health を走らせ、多くは捨てられる。
7. health verdict の形
   - A（採用）: `ReviewOutput.health` の固定 7 種の finding。blocking/advisory を finding ごとに持つ。
   - B: 自由文の reason の書式で表す。dispatcher が文字列を解析することになる。
8. policy の file を変える branch の通し方
   - A（採用）: policy の file だけの branch に分け、reviewer の policy verdict と人の承認で通す。評価は base 側の policy のまま。
   - B: `ReviewRequest.health` に載せて reviewer の verdict だけで通す。候補を作った側と同じ種類の agent だけで policy を弱められる。
   - C: WU の check では policy.changed を advisory にし、final review でだけ必須にする。check に新旧切替の分岐が増える。

## 新しい状態・分岐・fallback・port
- 状態
  - 「設計 review 待ちの計画」: 永続化するのは events だけ。`Event::DesignReviewRequested` があり、同じ `plan_sha256` の `Event::DesignReviewed` が無い間がこの状態（D3.5）。ほかの表・列は足さない。
  - health の材料: `Event::HealthReviewInputBuilt`（D4.2）。再利用の判定は events から行う。
- 分岐
  - planner_flow: risk が `high` なら設計 review、`low` なら今までどおり採用。
  - review_spawn: `[review]` があり review policy を読めたときだけ `ReviewRequest.health` を作る。
  - review_verdict: health の blocking finding を局所修復か replan に振り分ける（D5.3 の表）。
  - check-health.sh: policy の file だけの branch のとき exit 0 にして review に回す（D6.2）。
- fallback: 新しく置かない。review policy を読めない（script の error、remote workspace）ときは理由付きで `risk = unknown` にし、高 risk と同じ扱いにする（fail closed）。`[review]` 節が無い repo は `risk = none`（その repo は設計 review を使わない）で、event に理由 `review_policy_absent` を残す。どちらも黙って省かない。
- port（外への新しい口）
  - CLI: `repo_health.py --print-review-policy --repo <dir> --base <sha> [--profile <name|path>]`。出力は JSON `celeris.repo-health.review-policy/1`（schema は `scripts/dev/repo-health/schema/review-policy-v1.schema.json`）。
  - wire: `RunContext.design_review`、`ReviewRequest.health`、`ReviewOutput.design`、`ReviewOutput.health`（D3・D4・D5）。`PROTOCOL_VERSION` は 4 のまま（追加欄の慣例、protocol.rs:30-31）。`docs/protocol/worker-protocol.schema.json` を再生成する。
  - 計画: `ExecutionPlanSpec.design_brief`（/3 のみ）。`docs/protocol/execution-plan.schema.json` を再生成する。

## 撤去対象
D7 に決定として書く。要点: reviewer prompt の進捗ファイルの hash 規則の写し、planner prompt のうち `CLAUDE.md` と同じ文、`docs/guides/source-health.md` の重複した説明、reviewer prompt の Result 節の旧い形。

## 不変条件
- I1: 汎用規約の文は 1 つの prompt に 1 回だけ現れる。正本は `generic.md` の 1 file で、ほかの source に写しを置かない。
- I2: `generic.md` は repo 固有の名前（crate 名・`CLAUDE.md` の規則の本文・SQLite・events など）を含まない。repo 固有の規則は `CLAUDE.md` と profile にだけある。
- I3: dispatcher が使う review policy と repo health の policy は、どちらも base（target）側の内容から読む。候補の内容で自分の review の条件を変えられない（契約 D2）。
- I4: risk の判定は入力（review policy・write paths・brief）だけで決まる純粋関数で、LLM を呼ばない。
- I5: health の材料は `(repo_id, target_sha, reviewed_sha, definition_sha256)` ごとに高々 1 回作る。WU の check・tick では作らない。
- I6: health verdict の finding 種別は 7 つに固定。未知の種別は review.json の読み込みエラーとして扱う。
- I7: `ReviewRequest.health` を渡した review で `ReviewOutput.health` が無い・壊れていれば、その review は合格しない。
- I8: policy の file を変える candidate は、人の承認なしには合格しない。
- I9: 設計 review の待ちは events だけで表し、worker・dispatcher のメモリに持たない（`CLAUDE.md` の禁止の節）。

## 決定

### (1) 汎用規約の正本と配送
- D1.1 正本は `crates/task-worker/src/change_rules/generic.md` の 1 file。module は `crates/task-worker/src/change_rules.rs` に置き、`pub const GENERIC_RULES: &str = include_str!("change_rules/generic.md");` と `pub fn section() -> String`（見出し `## 変更の規約（汎用）` + 本文）を持つ。
- D1.2 組み立て口は `claude_code::prompt` だけ。`prompt_header` の preamble（`crate::preamble::render`）の直後に `change_rules::section()` を 1 回入れる。execute・plan・replan・execution plan・review の全 builder が `prompt_header` を通るので、Claude Code・Codex・Pi・ACP・Aider の全 harness に届く。adapter（`codex.rs`・`pi.rs`・`acp.rs`・`aider.rs`・`claude_code.rs`）では足さない。
- D1.3 CoS chat run（`build_cos_chat_parts`）には入れない。CoS は作業ツリーを変えず、判断の規則は CoS の skill と policy_version が持つ。
- D1.4 `generic.md` の中身は repo に依らない一般形にする: 原因の owner で直す、同じ仕様を二重に実装しない、共通 interface に局所の hook を足さない、状態の正本を 1 つにする、承認の無い fallback・新旧切替を置かない、置き換えた旧経路を同じ変更で消す、試験の追加・削除に理由を書く、の 7 項目。D5.1 の finding 種別と同じ順・同じ名前にする。
- D1.5 `CLAUDE.md`・`docs/guides/source-health.md` と重なる文は写さない。`generic.md` は「repo の必須規則と健全性の guide に具体があれば、それに従う」の 1 文で参照するだけにする。`CLAUDE.md` の「変更の方針」「禁止」節は Celeris の必須規則の正本のまま残す（ADR 2026-10-23。Codex・Pi・ACP にも写される）。`source-health.md` は D7.3 で ADR と入口への参照に縮める。
- D1.6 Aider に `CLAUDE.md` が届かない穴（survey (a)）はこの ADR の範囲の外とし、進捗の提案に回す。

### (2) repo 固有部: profile の `[review]` 節と base 側の読み込み
- D2.1 `profile.py` の `_TOP_KEYS` に `review` を足し、docstring（項目の正本）に書く。未知の下位項目は読み込みエラー。
- D2.2 節の形（`profiles/celeris.toml` に置く値の例）:
  ```toml
  [review]
  owner_map = "docs/architecture-map.md"   # 変更 path の owner 行を引く file（地図の表の行）

  [[review.high_risk]]
  class = "dispatch-state"                 # 高 risk の path class の名前
  paths = ["^crates/task-dispatch/src/dispatcher/", "^crates/task-core/src/model/", "^crates/task-store/"]

  [review.invariants]                      # 保護する不変条件の名前 → 根拠の file
  task-state-in-sqlite = "CLAUDE.md"
  events-append-only = "CLAUDE.md"
  no-llm-in-dispatcher = "CLAUDE.md"
  ```
  具体的な class と path の値は U2 が B0 の capability-map と architecture-map から決める。
- D2.3 dispatcher は `repo_health.py --print-review-policy --repo <dir> --base <target_sha>` を呼び、JSON `celeris.repo-health.review-policy/1`（`source`: base|bootstrap|builtin|absent、`owner_map`、`high_risk[{class, paths}]`、`invariants{name: source}`、`definition_sha256`）を読む。`definition_sha256` は解決した profile の blob の内容から作る。候補（head）の profile は読まない。base に `[review]` が無いときは `source = absent`。
- D2.4 Rust 側の型は `task_core::design_review::ReviewPolicy`（`owner_map: Option<String>`・`high_risk: Vec<PathClass { class, paths }>`・`invariants: BTreeMap<String, String>`・`definition_sha256: String`）。Rust では TOML を読まない。

### (3) 設計 brief・risk の判定・設計 review
- D3.1 型は `task_core::execution_plan::DesignBrief`（`deny_unknown_fields`）。欄:
  `root_cause: String`・`owner: String`・`reuse: Vec<String>`・`options: Vec<DesignOption { name: String, chosen: bool, reason: String }>`・`new_state: Vec<String>`・`new_branch: Vec<String>`・`new_fallback: Vec<String>`・`new_port: Vec<String>`・`removals: Vec<String>`・`invariants: Vec<String>`。
  `ExecutionPlanSpec.design_brief: Option<DesignBrief>`（/3 のみ。`skip_serializing_if = "Option::is_none"`。/1・/2 で書けば `PlanValidationError`）。
- D3.2 形の検証は `validate_v3` に置く（brief があるときだけ）: `root_cause`・`owner` が空でない、`options` が 2 件以上で `chosen = true` がちょうど 1 件、各要素の文字列が空でない、`invariants` の名前は自由文でよい。エラーは `PlanValidationError::DesignBrief { field, reason }`。
- D3.3 risk の判定は `task_core::design_review::assess_risk(policy: Option<&ReviewPolicy>, write_paths: &[String], brief: Option<&DesignBrief>) -> RiskAssessment { level: RiskLevel, reasons: Vec<String> }`。`RiskLevel` は `None | Low | High | Unknown`。
  - write paths は計画の全 unit の `expected_write_paths` と leaf の `context` の path の和。
  - policy が `absent` → `None`（理由 `review_policy_absent`）。policy を読めない → `Unknown`（理由 `review_policy_unavailable:<要約>`）。
  - write path が `high_risk` のどれかの class に合い、かつ brief が `new_state`・`new_branch`・`new_fallback`・`new_port` のどれかを宣言している → `High`（理由 `path_class:<class>+new_state` など）。
  - brief が `new_fallback` を宣言している → path に依らず `High`（理由 `new_fallback`）。
  - write path が高 risk の class に合い、brief が無い → planner の計画では `PlanValidationError::DesignBriefMissing { classes }` で planner に差し戻す（既存の検証失敗の再試行と同じ経路）。人の計画（task-ops）は brief を必須にせず、無ければ `Low` として理由 `brief_absent_human_plan` を残す。
  - それ以外 → `Low`。
- D3.4 brief の書き方は `build_execution_plan_prompt` に 1 節足し、planner に「高 risk の path class を変える計画では brief を書く」と review policy の class 一覧を渡す（`RunContext` の planner 欄に `review_policy: Option<ReviewPolicy>` を足す）。
- D3.5 設計 review の場所は `planner_flow.rs` の検証（:158）と `tree_plan_gate` の後、`adopt_plan_with_children`・`replan` の前。`High` と `Unknown` のとき:
  1. `Event::DesignReviewRequested { plan_sha256, plan: ExecutionPlanSpec, risk: RiskAssessment, policy_sha256 }` を追記する（計画の本文は event に持つ）。
  2. `pick_reviewer` で reviewer を選び、`run_reviewer_inner` と同じ形で reviewer run を起こす。`RunContext.review = None`、`RunContext.design_review = Some(DesignReviewRequest { brief, plan_summary, risk, review_policy })`。prompt は `build_review_prompt` の中で `design_review` があれば設計 review の節を出す。
  3. reviewer は review.json に `ReviewOutput.design: Option<DesignVerdictOut { pass: bool, findings: Vec<HealthFindingOut> }>` を書く（finding 種別は D5.1 と同じ 7 つ）。
  4. `Event::DesignReviewed { plan_sha256, review_run, pass, findings }` を追記する。`pass` なら event の計画を採用する。不合格なら planner を replan で起こし、findings を planner の prior に渡す。
  5. 同じ task で設計 review の不合格が 2 回続いたら `Trigger::PlanGate` で人に回す（`PlanApproval.reasons` に `design_review_failed:<n>`）。
- D3.6 `Low` と `None` は今までどおり採用する。`root_plan_approval` の判定は変えない。

### (4) 実装後の `ReviewRequest.health` を candidate SHA ごとに 1 回
- D4.1 型（protocol.rs）: `ReviewRequest.health: Option<HealthReviewInput>`（`serde(default, skip_serializing_if = "Option::is_none")`）。
  `HealthReviewInput { repos: Vec<HealthRepoInput>, brief: Option<DesignBrief> }`。
  `HealthRepoInput { repo_id, target_sha, reviewed_sha, diff: DiffStat { files, insertions, deletions, paths: Vec<PathStat> }, owner_rows: Vec<OwnerRow { path, row }>, findings: HealthFindingsSummary { verdict, reviewable, counts, new: Vec<HealthReportFinding { rule_id, fingerprint, severity, delta, path }> , truncated: usize }, policy: HealthPolicyState { source, changed, files }, invariants: BTreeMap<String, String> }`。
  - `findings.new` は report の `findings[]` のうち `delta ∈ {new, worsened}` かつ `exception == null` のもの。上限 50 件、`paths` と `owner_rows` は上限 40 件。超えた数を `truncated` に書く。
  - `owner_rows` は base 側の `owner_map` file から、変更 path を含む表の行を写す。
  - `brief` は採用した計画の `design_brief`（reviewer は宣言した `removals` が実際に消えたかを確かめる）。
- D4.2 作る場所は `crates/task-dispatch/src/dispatcher/review_health.rs` の `build_health_input`。`spawn_review` が `ReviewSync` を作った後（ADR-0118 の同期の後）に呼ぶ。repo health は `repo_health.py --repo <dir> --base <target_sha> --head <reviewed_sha> --policy-version 1 --out <daemon の state dir>` で 1 回走らせる。`summary.reviewable == false` の report は材料にせず、`findings` を空にして理由を残す。
- D4.3 結果は `Event::HealthReviewInputBuilt { review_run, repo_id, target_sha, reviewed_sha, definition_sha256, input }` に追記する。同じ `(repo_id, target_sha, reviewed_sha, definition_sha256)` の event があれば作り直さず、その `input` を使う（reviewer run の再試行・失敗後の再 review）。`reviewer_reuse` で reviewer を省くときは作らない。
- D4.4 WU の check・WU の review・tick・remote workspace では作らない。remote workspace と review policy が `absent` の repo では `ReviewRequest.health = None` にし、理由を `HealthReviewInputBuilt` でなく `Event::HealthReviewInputSkipped { review_run, repo_id, reason }` に残す。

### (5) `ReviewOutput.health` の構造化 verdict と repair/replan への写像
- D5.1 型（protocol.rs）: `ReviewOutput.health: Option<HealthVerdictOut>`、`HealthVerdictOut { findings: Vec<HealthFindingOut>, policy: Option<PolicyVerdictOut> }`、`HealthFindingOut { kind: HealthFindingKind, severity: HealthSeverity, path: Option<String>, reason: String, repair: Option<ReviewRepairHint> }`、`HealthSeverity = blocking | advisory`、`PolicyVerdictOut { approved: bool, reason: String }`。`HealthFindingKind` は serde の snake_case で固定 7 種:

  | kind | 内容 | blocking にする条件 | blocking の写像 |
  |---|---|---|---|
  | `symptom_fix_outside_owner` | owner の外で症状を直した | owner 地図の行と直した場所が違い、原因が残る | substantive → replan |
  | `duplicate_implementation` | 同じ仕様の二重実装 | 既存の実装と同じ入出力の関数・経路を新しく置いた | substantive → replan |
  | `local_hook_in_shared_interface` | 共通 interface への局所 hook | 共通の型・trait・関数に特定の呼び出し元だけの分岐を足した | substantive → replan |
  | `dual_source_of_truth` | 状態の二重正本 | 同じ状態を 2 か所に持ち、どちらを正とするかが決まっていない | substantive → replan |
  | `unapproved_fallback_or_switch` | 未承認の fallback・新旧切替 | brief・ADR に無い fallback、新旧切替の flag・互換分岐 | substantive → replan |
  | `removal_missing` | 旧経路の撤去漏れ | 置き換えた旧い経路・欄・試験・文書が残っている | `repair.scope = "local"` があれば局所修復、無ければ replan |
  | `test_change_unjustified` | 試験の追加・削除の根拠が無い | 試験を消した・弱めた理由が差分・進捗に無い | `repair.scope = "local"` があれば局所修復、無ければ replan |

  advisory の finding は合否に使わず、`Event::ReviewVerdict` の記録と次の run の `prior_review` に載せる。
- D5.2 読み込み（review.rs）: `ReviewRequest.health` を渡した review で `output.health` が無い、または未知の kind・severity を含めば、既存の `fail_all` と同じく全 criterion を不合格にし、reason を `{tag}: health verdict missing or invalid in review.json` にする（I7）。
- D5.3 写像（review_verdict.rs）: blocking の finding 1 件ごとに `Verdict { criterion_idx, pass: false, reason: "{tag}: health:<kind>: <reason>", repair_hint }` を足す。`criterion_idx` は `ReviewRequest.criteria` の最小の index（reviewer の受け入れ条件）。`repair_hint` は D5.1 の表で局所修復を許す 2 種だけ写し、ほかの 5 種は `None` にする。これで `classify_review_failure` は既存の規則のまま、全部が局所なら `RepairClass::ReviewerLocal`、1 件でも substantive なら `Trigger::ReviewFail` → replan になる。reason が `reviewer(<id>): …` の形なので CoS の `review_rework` の検査も変えない。
- D5.4 reviewer prompt の Result 節（prompt.rs:1785-1791）を作り直し、`verdicts[].repair`・`health`・`design` の形と 7 種の表を 1 か所で出す。種別の説明文は `generic.md` の 7 項目を参照する。

### (6) policy の file を変える branch の通し方
- D6.1 policy の file（profile の `policy_paths`）を変える変更は、policy の file だけを変える task（branch）に分ける。candidate の report が `policy.changed` で、変更 path に policy の file 以外も含むとき、dispatcher は blocking と同じ扱いの `Verdict { reason: "{tag}: health:policy_mixed: …" }`（substantive → replan）を足す。planner は replan で policy の変更を別の task に分ける。
- D6.2 `scripts/dev/check-health.sh` は、fail 級の finding が `policy.changed` だけで、`merge-base..HEAD` の変更 path が全部 `policy.files` に入るとき、exit 0 にして `policy review required: <files>` を出す。report の verdict は fail のまま（契約は変えない）。ほかの rule は base 側の policy で評価したまま。
- D6.3 その task の review では `HealthRepoInput.policy.changed = true` になり、reviewer は `HealthVerdictOut.policy` を必ず書く（無ければ D5.2 と同じく不合格）。`approved = false` は substantive。
- D6.4 `approved = true` でも、dispatcher は人の承認を要求する: `Check::Human` の既存の経路で「policy の変更の承認」を待ち、受信箱に policy の file と diff の要約を出す（I8）。人が承認したら合格、却下は substantive。
- D6.5 main に入った後の branch は、新しい main を base にして新しい policy で評価される（契約 D2 のまま）。
- D6.6 移行: G2 の実装のうち `scripts/dev/repo-health/`・`check-health.sh` を変える U2 は、それ自体が policy の file だけの branch になる。main の base には D6.2 の扱いがまだ無いので、U2 の最初の branch は D6.4 と同じ人の承認で通す（task の受け入れ条件に `{"type":"human"}` を置く）。`source-health.md:34-35` の「基準・閾値の変更は独立レビュー」はこの (6) を指す。

### (7) 撤去する旧い仕組み
- D7.1 `build_review_prompt` の「Commit references in progress files and reports」節（prompt.rs:1645-1655）を消す。同じ規則は `CLAUDE.md` にあり、全 harness に届く（Aider を除く。D1.6）。
- D7.2 reviewer prompt の Result 節の旧い形（`{"verdicts":[{"criterion","pass","reason"}]}` だけを指示する文）を D5.4 の 1 節に置き換える。旧い形の文と、それを固定する試験を消す。
- D7.3 `docs/guides/source-health.md` を入口と ADR への参照に縮める: 入口・base・policy の扱いは `check-health.sh` の usage と [repo health の契約](2026-11-07-repo-health-contract.md)・[Celeris profile](2026-11-07-repo-health-celeris-profile.md)、基準の変更の review はこの ADR の (6) を参照する。同じ変更で front matter を先頭に移し、:45 の link を実在する path に直す。`check-health.sh` の冒頭注記と `profile.py` の docstring のうち ADR と同じ説明は ADR への参照にする（`profile.py` の docstring は profile の項目の正本として残す）。
- D7.4 `PLANNER_CHECK_GUIDANCE`（prompt.rs:1797-1870）から `CLAUDE.md` と同じ文（記録の場所、全体試験は release gate だけ、CPU を焼く負荷の禁止、hash を書かない）を消す。`CELERIS_WU_*`・scope check の書き方は Celeris の検査機構の手順で `CLAUDE.md` に無いので残す。`PRODUCTION_HOST_PLANNER_GUIDANCE`（:1874）は、planner の prompt に `preamble::fixed_notes` の production host の注意が入ることを U6 の試験で確かめたうえで消す。
- D7.5 残すもの: reviewer prompt の policy violation の節（prompt.rs:1718-1735）は `ReviewRequest.policy_violations` の構造化された材料の説明で、ほかに写しが無い。
- D7.6 新しい仕組みに切替 flag・無効化の config 欄を置かない。health の材料と verdict は `[review]` 節のある repo では常に使う。

### 後続 unit の path・型名・試験名
試験名の prefix は Rust の試験関数名・Python の `test_` の後に付ける。
- U1 change_rules: `crates/task-worker/src/change_rules/generic.md`（新規）、`crates/task-worker/src/change_rules.rs`（新規。`GENERIC_RULES`・`section()`）、`crates/task-worker/src/claude_code/prompt.rs`（`prompt_header`）。試験 prefix `change_rules_`（例 `change_rules_generic_in_every_harness_prompt`・`change_rules_generic_appears_once`・`change_rules_absent_from_cos_chat`・`change_rules_generic_has_no_repo_specific_names`）。
- U2 review policy（policy の branch。D6.6）: `scripts/dev/repo-health/health/profile.py`、`scripts/dev/repo-health/repo_health.py`（`--print-review-policy`）、`scripts/dev/repo-health/schema/review-policy-v1.schema.json`、`scripts/dev/repo-health/profiles/celeris.toml`（`[review]`）、`scripts/dev/check-health.sh`（D6.2）。試験 `scripts/tests/test_repo_health_review_policy.py`、prefix `test_health_review_`（例 `test_health_review_policy_reads_base_side`・`test_health_review_policy_only_branch_exits_zero`）。
- U3 設計 brief と risk: `crates/task-core/src/execution_plan.rs`（`DesignBrief`・`DesignOption`・`ExecutionPlanSpec.design_brief`）、`crates/task-core/src/execution_plan/validation.rs`（`PlanValidationError::DesignBrief`・`DesignBriefMissing`）、`crates/task-core/src/design_review.rs`（`ReviewPolicy`・`PathClass`・`RiskLevel`・`RiskAssessment`・`assess_risk`）、`crates/task-core/src/model/events.rs`（`DesignReviewRequested`・`DesignReviewed`）、`crates/task-dispatch/src/dispatcher/planner_flow.rs`、`crates/task-worker/src/protocol.rs`（`RunContext.design_review`・`DesignReviewRequest`・`ReviewOutput.design`・`DesignVerdictOut`）、`docs/protocol/execution-plan.schema.json`。試験 prefix `design_brief_`（例 `design_brief_requires_one_chosen_option`・`design_brief_high_risk_path_with_new_state_is_high`・`design_brief_missing_on_high_risk_path_is_rejected`・`design_brief_review_runs_before_adoption`）。
- U4 health の材料: `crates/task-worker/src/protocol.rs`（`ReviewRequest.health`・`HealthReviewInput`・`HealthRepoInput`・`DiffStat`・`PathStat`・`OwnerRow`・`HealthFindingsSummary`・`HealthReportFinding`・`HealthPolicyState`）、`crates/task-dispatch/src/dispatcher/review_health.rs`（`build_health_input`）、`review_spawn.rs`、`events.rs`（`HealthReviewInputBuilt`・`HealthReviewInputSkipped`）、`docs/protocol/worker-protocol.schema.json`。試験 prefix `health_review_`（例 `health_review_input_built_once_per_candidate`・`health_review_input_reused_on_reviewer_retry`・`health_review_input_not_built_for_work_units`）。
- U5 health verdict: `crates/task-worker/src/protocol.rs`（`ReviewOutput.health`・`HealthVerdictOut`・`HealthFindingOut`・`HealthFindingKind`・`HealthSeverity`・`PolicyVerdictOut`）、`crates/task-dispatch/src/review.rs`、`crates/task-dispatch/src/dispatcher/review_verdict.rs`、`prompt.rs` の Result 節。試験 prefix `health_review_`（例 `health_review_verdict_missing_fails_review`・`health_review_removal_missing_local_repairs`・`health_review_dual_source_replans`・`health_review_policy_change_waits_for_human`）。
- U6 撤去: `prompt.rs`（D7.1・D7.2・D7.4）、`docs/guides/source-health.md`、`scripts/dev/check-health.sh` の冒頭注記（D7.3。policy の file なので U2 と同じ branch に入れる）。試験 prefix `change_rules_`（例 `change_rules_review_prompt_has_no_commit_hash_copy`）。
- 新しい module の inline 試験が source-size gate の上限に近づくなら、`<module>/tests.rs` を `#[path]` で分ける。

## 付記（2026-11-08、dispatch の実装）
task 01M6VTFG9K5HEE817EW05JASMZ（dispatcher: risk 判定・高 risk 設計 review・final review の health 文脈と構造化 verdict）で D2.3・D3.3・D3.5・D3.6・D4・D5.2・D5.3・D6.1・D6.3・D6.4 を実装した。記録は [dispatch の進捗](../progress/2026-11-07-health-v2-g2/dispatch.md)。上の決定節との違いを次に書く。

### 置き場所: `dispatcher/review_health.rs` でなく `crates/task-dispatch/src/health_review/`
- D4.2 の `dispatcher/review_health.rs`、D5.3 の `review_verdict.rs` の写像、D3.5 の `planner_flow.rs` の中の処理は、新しい top-level module `crates/task-dispatch/src/health_review/` に置いた。理由は repo health の `size.responsibility-growth`。この rule は Rust の module を `<pkg>/src/<第 1 要素>` の単位で数え、`crates/task-dispatch/src/dispatcher/` は base の時点で module 全体が `module_lines = 8000`（`scripts/dev/repo-health/profiles/celeris.toml`）を超えている。`dispatcher/` に file を足すと worsened になり、`check-health.sh` が落ちる。
- `health_review/{policy,design,input,verdict}.rs` は top-level の下位 module。Dispatcher の private な状態（実行中の run・完了の channel）を使う `design_gate.rs`・`design_verdict.rs` は `dispatcher.rs` の、`plan_adoption.rs` は `dispatcher/planner_flow.rs` の、`policy_review.rs`（D6.4）は `dispatcher/child_tasks.rs` の子 module として `#[path]` で読む。file は `health_review/` にあるので、行は `health_review` の module に数えられる。
- `dispatcher/` に足したのは呼び出しの配線だけで、その行は同じ file の既存の行を移して相殺した（`on_planner_finished` の run の終わり方の写像を `design_gate.rs` の `run_end_of` へ、計画の採用の手順を `plan_adoption.rs` へ。後者は設計 review を通った計画の採用と共通にし、二重実装を避ける）。
- health verdict の写像は `health_review/verdict.rs` の `impl Verdict` に置き、`review.rs` からは `Verdict` の関数として呼ぶ。`crate::health_review::…` の path で呼ぶと `review` → `health_review` → `dispatcher` → `review` の循環になり、`deps.cycle` が worsened になる。

### 設計 review の wire 欄の置き場所
- D3.5 2. の `RunContext.design_review` は作らず、`ReviewRequest.design: Option<Box<DesignReviewRequest>>`（`context.review.design`。`crates/task-worker/src/protocol.rs`）に置いた。`RunContext` の直下は既に 40 欄で、`type_fields = 40` の閾値に当たる。設計 review の run は reviewer run なので、`context.review` が `Some` の run への既存の扱い（browser を渡さない判定など）がそのまま効く。
- D3.4 の planner 欄 `review_policy` も `RunContext` の直下でなく `ExecutionPlannerContext.review_policy` に置いた（同じ理由）。
- `DesignReviewRequest.brief`・`review_policy` と `DesignReviewRequested.policy_sha256` は `Option`（方針を読めない `unknown` では値が無い）。`DesignReviewRequested` は方針の本文を持たないので、dispatcher は reviewer run を起こすときに base 側から読み直し、`definition_sha256` が event の `policy_sha256` と同じときだけ渡す。
- `DesignReviewed.findings` と `HealthReviewInputBuilt.input` は `serde_json::Value`（wire 型の JSON の写し）。wire 型は task-worker にあり、task-core は task-worker に依存しないため、task-core に同じ形の型を二重に置かない。
- `DesignBrief`・`DesignOption` は `task_core::design_review` に置き、`task_core::execution_plan` から `pub use` する（module の循環を避けるため）。`assess_risk` の第 1 引数は `Option<&ReviewPolicy>` でなく `&ReviewPolicyState`（`Present`・`Absent`・`Unavailable(理由)`）で、absent と読めないを区別する。path class の正規表現を Rust の `regex` で読めないときは `unknown`（fail closed）。

### 試験名の prefix
- 上の「後続 unit の path・型名・試験名」は U3 を `design_brief_`、U4・U5 を `health_review_` に分けていたが、dispatcher の試験は設計 review を含めて全部 `health_review_` にした（task の目的 (6)）。関数名が `health_review_` で始まるものは task-dispatch に 68 件（うち下の「policy の関門の fail closed」の `health_review_policy_unknown_` が 6 件、`health_review_policy_no_reviewer_` が 7 件）、`crates/task-core/src/design_review/tests.rs` に 9 件、`crates/task-worker/src/protocol/design_review/tests.rs` に 4 件ある。brief の形の検証（`crates/task-core/src/execution_plan/design_brief_tests.rs`）は `design_brief_` のまま。
- 流れ全体の試験は `crates/task-dispatch/src/health_review/scenario_tests.rs` の `health_review_scenario_normal_change`（追加の run なし・health 文脈だけ）・`health_review_scenario_high_risk_change`（設計 review が起き、blocking で不合格）・`health_review_scenario_real_cli`（base の本物の `--print-review-policy`）。`dispatcher::tests` の private な道具を使うので、`dispatcher/tests/mod.rs` から `#[path]` で読む。

### 検査器も base の版を使う
- D2.3・D4.2 は base 側の profile を読むと書いたが、検査器（`repo_health.py` と同じ dir の module）も base の版を使う（I3）。`health_review/policy.rs` の `with_base_checker` が、target を `git rev-parse --verify <target>^{commit}` で commit に解決し、`git cat-file -e` で base に `scripts/dev/repo-health/repo_health.py` があるかを確かめ、`git archive` で base の `scripts/dev/repo-health/` と `scripts/dev/source-size-report.py` を一時 dir に出して展開し、その script を起こす。方針の読み込みは `--print-review-policy --repo <repo> --base <target_sha>`、health 文脈は `--repo <repo> --base <target_sha> --head <reviewed_sha> --policy-version 1 --out <dir>`。候補（head・作業ツリー）の検査器と profile は使わない。
- base に検査器が無い repo は `Unavailable` でなく `Absent`（risk は `none`、health 文脈は `HealthReviewInputSkipped`）にした。候補が検査器を足しても base には無いので、候補は自分の条件を変えられない。`Unavailable` にすると、検査器を持たない repo（Celeris 以外の案件の repo）の計画がすべて設計 review に回る。
- 時間の上限は方針の読み込みが 20 秒（`REVIEW_POLICY_TIMEOUT`）、health 文脈が 600 秒（`HEALTH_INPUT_TIMEOUT`）。health 文脈は reviewer run を起こす非同期 task の中（`spawn_blocking`）で作り、tick を止めない。

### そのほかの違い
- D3.5 の待ち: 設計 review の間、task は `ready` に戻り、`dispatch_one` の hold が planner・WU を起こさない。新しい状態・列は足していない（I9）。`done` で終わらない設計 review の run（adapter の失敗・中断・質問・予算切れ）は不合格に数えず、`[review] max_reviewer_retries` まで起こし直す。超えたら `DesignReviewed { pass: false, findings: [] }` を書く。
- D3.5 4. の「replan で起こす」は、planner の既存の再試行（`WorkerProgress` の再試行の文 → `planner_retry_pending`）で表した。findings はその文に入り、planner の prior になる。D3.5 5. の `PlanGate` は、その計画を採用してから引く（`Trigger::PlanGate` は採用した計画の id を要る）。
- risk の level の順位は none < low < unknown < high。repo ごとの判定のうち最も高いものを採る。
- D4.3 の「高々 1 回」は policy の状態が分かった材料について。repo health の report を作れなかった（report を書かない・時間切れ・壊れた report・schema 違い）材料は、reviewer に `findings.verdict = "error"` と理由を渡し、同じ材料を `policy.source = "unavailable"`（policy の状態が不明）として `HealthReviewInputBuilt` に記録する。不明の材料は再利用せず、次の review で report を作り直す。不明の記録は下の「policy の関門の fail closed」により人の承認を求める。reviewer の `verdict = error` を見た判定だけで候補が合格することは無い。
- D5.3 の「ほかの 5 種は `repair_hint = None`」は `Some({scope: "substantive", class: <kind>})` にした。`None` だと `classify_reviewer_reason` が reason の文面（`format`・`lint` などの語）から局所修復と推測しうる。写像の結果（局所修復か replan か）は D5.1 の表のとおり。
- D5.1 の advisory は合格の `Verdict` として `Event::ReviewVerdict` に残し、reviewer 自身の判定より前に置く（同じ criterion の最後の verdict を読む task-ops が advisory を判定と読まないため）。新しい event の種類は足していない。
- D6.1 の `policy_mixed` は `review.json` を読めた review にだけ足す。決定的な検査が落ちて reviewer を起こさなかった review には足さない（その不合格は既に substantive で replan に回る）。
- D6.4 の人の承認: `Approval` の子 task の題は candidate の SHA で鍵を作る（同じ candidate の再 review は同じ子を使い、SHA が変われば新しい承認を求める）。待ちは既存の子待ちと `resolve_human_approvals` の延期を使い、子が終端になったら review をやり直す。却下の reason は `human(approval task <id>):` で始め、CoS の `review_rework` の対象にしない。
- planner run に渡す方針（`ExecutionPlannerContext.review_policy`）は task の先頭の repo の 1 つだけ。計画の採用前の判定（`design_gate`）は repo ごとに方針を読む。
- この task の範囲外で未実装: D3.4 の `build_execution_plan_prompt` の brief の節、D5.4 の reviewer prompt の Result 節（`health`・`design` の形と 7 種の表）、reviewer prompt の health 文脈・設計 review の依頼の節。prompt の組み立ては task-worker の prompts task（01M6VTFGWFVE57YXDKHR2E436G）が受け持つ。

### policy の関門の fail closed（final review の差し戻しの修正）
final review は、I8 と (1) の「fallback: 新しく置かない。読めないときは fail closed」に反する穴を 3 つ見つけて差し戻した。(a) base の検査器が report を作れないと、policy を `changed = false` として reviewer に渡し、`HealthReviewInputBuilt` も記録しなかった。(b) final review で base の方針を読めない（`Unavailable`）repo は `HealthReviewInputSkipped` を残すだけで、判定に使わなかった。(c) `Check::Reviewer` の条件が無い task は health 文脈を作らず、関門も掛からなかった。policy の file だけの候補は D6.2 で `check-health.sh` が exit 0 になるので、どれも人の承認なしに合格しえた。WorkUnit policy-unknown と policy-no-reviewer で次のように直した。
- 関門の対象（D6.4 の範囲を広げる）: base に検査器（`scripts/dev/repo-health/repo_health.py`）がある repo の candidate は、policy の状態を candidate `(repo_id, target_sha, reviewed_sha)` ごとに event に残す。`dispatcher::policy_review` の `policy_review_gate` がそれを読み、状態ごとに次のように扱う。
  - 変更なし（`changed = false`）: 承認を求めない。
  - 変更あり（`changed = true`）: D6.4 のとおり人の承認を求める。
  - 不明（`policy.source = "unavailable"`）: 変更ありと同じに扱い、D6.4 の人の承認を求める（fail closed）。
- 不明になる場合: base の方針を読めない（`Unavailable`・`--print-review-policy` の失敗）、base の検査器を出せない（`with_base_checker` の失敗。target を解決できないなど）、diff を取れない、report を作れない（report を書かない・時間切れ・壊れた report・schema 違い）。
- 記録の場所: 新しい Event の種類・状態・config 欄・切替 flag は足していない。reviewer に材料を渡した repo は `HealthReviewInputBuilt.input.policy` に、渡さなかった repo は `HealthReviewInputSkipped` に serde default で足した欄 `target_sha`・`reviewed_sha`・`policy` に残す。同じ candidate に既知（変更あり・なし）と不明の記録があれば既知を使う。base（`target_sha`）が同じなら検査器と方針も同じで、不明は一時的な失敗と見なせるため。中身を読めない記録も不明に数える。
- `[review]` が無いが検査器のある repo（方針 `Absent`）: reviewer に渡す health 文脈は D4.4 のとおり作らない（Skipped）。そのうえで report を 1 回走らせ、policy の状態を `HealthReviewInputSkipped.policy` に残して関門に掛ける。base に検査器が無い repo だけは従来どおり関門が無い（候補が検査器を足しても base には無いので、候補は自分の条件を変えられない）。
- `Check::Reviewer` の無い task（reviewer run の無い final review）: `Dispatcher::health_job` が `policy_only` の仕事を作る。review の非同期 task の中で base の検査器の report を candidate ごとに 1 回走らせ、policy の状態を `HealthReviewInputSkipped`（reason は定数 `NO_REVIEWER`、`review_run` は worker run の id）に残す。reviewer には何も渡さず、方針（`--print-review-policy`）も読まない（方針は reviewer の材料にしか使わないため）。`ReviewerPassReused` で reviewer を省いた candidate には仕事を作らず、関門は元の candidate の記録を見る。同じ candidate の既知の記録があれば repo health を走らせない。
- verdict を付ける criterion（`policy_criterion`）: `Check::Reviewer` の条件があればその条件（D5.3 と同じ）。無ければ、この review の verdict の最大の `criterion_idx` の次（acceptance の外）に付ける。`Check::Command` の条件に付けると、却下の不合格を `try_review_repair` が cmd で分類し、局所修復（fmt・clippy など）に変えうるため。acceptance の外の不合格は `Check::Human` と同じく修復しない（substantive → replan）。
- 不明には D6.3 の reviewer の policy verdict（`HealthVerdictOut.policy`）を求めない。reviewer は report を見ていないので policy の変更を判定できない。判定は人の承認だけで決める。承認の子の本文には `policy state unknown: <理由>` を、承認・却下の verdict の reason には `policy state unknown (<理由の先頭 300 字>)` を載せる。人が承認したら合格、却下は substantive（D6.4 と同じ）。
- D6.1 の `policy_mixed` は、上の「そのほかの違い」のとおり `review.json` を読めた review にだけ足す。reviewer の無い review で policy の file とほかの file を混ぜた候補は、`policy_mixed` の replan でなく人の承認を求める。人が却下すれば replan になる。
- 時間: reviewer の無い final review は、検査（`review_task`）の前に base の検査器の report を待つ（上限は `HEALTH_INPUT_TIMEOUT` の 600 秒）。base に検査器がある repo（この repo など）では、`Check::Reviewer` の無い task の final review が repo health の実行時間だけ遅れる。同じ candidate の 2 回目以降は記録を使うので遅れない。
- 試験名の prefix は `health_review_policy_unknown_` と `health_review_policy_no_reviewer_`。前者は `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs` に 5 件、`crates/task-dispatch/src/dispatcher/tests/health_review_input.rs` に流れ全体の 1 件。後者は `crates/task-dispatch/src/health_review/policy_review/no_reviewer_tests.rs` に 5 件、`crates/task-dispatch/src/dispatcher/tests/health_review_input.rs` に流れ全体の 2 件。
- 残る穴（未実装）: 材料作りの非同期 task が panic した（`spawn_blocking` の失敗）ときは event が残らず、関門が掛からない。同期の無い repo（同期を省いた・失敗した）と remote workspace の repo は candidate の SHA が決まらないので、記録も関門も無い（D4.4 のとおり）。
