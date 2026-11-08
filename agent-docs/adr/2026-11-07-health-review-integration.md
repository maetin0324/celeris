---
tasks: [01M6VBSKZQG2YWF7NWYH6GP9P7]
---
# ADR 2026-11-07: 健全性を設計と review に組み込む（規約配布・設計 brief・risk 判定・構造化 health verdict）

- 日付: 2026-11-07
- 状態: 実装済み（2026-11-08、健全性 v2 G2 の task 01M6TYVJSTYGRPHERB93MM7J59。二度目と三度目の final review の差し戻しの修正を含む。記録は [G2 の進捗](../progress/2026-11-07-health-v2-g2.md)）。決定節との食い違いは末尾の付記（dispatch の実装・G2 の完了時の突き合わせ）に、二度目の差し戻しへの方針と決定を改めた点（D7.1・D7.4）は「二度目の final review の差し戻しへの方針」の付記に、その方針と実装の違いと残る穴は付記「差し戻しの修正の完了時の突き合わせ」に、三度目の差し戻し（architecture-map の 2 行と doc comment の旧い名前）の修正は最後の付記「三度目の final review の差し戻しの修正」に書く。
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

## 付記（2026-11-08、G2 の完了時の突き合わせ）
G2 の全 unit（adr・guideline・plan-brief・protocol・dispatch・prompts）を統合した後の HEAD で、決定節を実装と突き合わせた。上の「付記（dispatch の実装）」に書いた違いはここで繰り返さない。その付記の「この task の範囲外で未実装」のうち、D3.4 の brief の節（`change_rules::design_brief_section`）、reviewer prompt の health の節、Result 節の `health` の形は prompts task（01M6VTFGWFVE57YXDKHR2E436G）が実装した。残りは下の「未実装」に書く。記録は [G2 の進捗](../progress/2026-11-07-health-v2-g2.md)。

### (1) 汎用規約
- D1.1: module は `crates/task-worker/src/change_rules.rs`。`generic.md` を `include_str!` で埋め込む。reviewer の health の節と Result 節は子 module `change_rules/review.rs`（`health_section`・`result_section`）に置いた。`prompt.rs` が 2,000 行の上限に近く（約 1,880 行）、描画を足せないため。
- D1.4: `generic.md` は `## 規約` 10 項目と `## 実装前 brief` 7 項目にした（決定節は 7 項目）。task の受け入れ条件が 10 項目を求めたため。D5.1 の 7 kind に当たるのは規約 1〜6 と 10 で、kind 名を括弧で付けて D5.1 の表と同じ順に置いた。規約 7〜9（移行の撤去条件と責任者・共通化は同じ意味のものだけ・範囲で根本修正できなければ repair/replan）に対応する kind は無く、違反の返し方は未決。
- D1.2: `prompt_header` は汎用規約の直後に repo 固有の review 方針の節を出す。方針は `ExecutionPlannerContext.review_policy` にしか無いので、この節が出るのは /3 の planner の run だけ。worker の run には出ない。reviewer には方針の不変条件と owner 行が `HealthRepoInput` で届く。
- U1 の試験名は例と違う（`change_rules_generic_reaches_{claude_code,codex,pi,acp}_…`・`change_rules_generic_once_in_every_run_kind`・`change_rules_generic_and_required_rules_do_not_copy_each_other` など。`change_rules_` は 16 件）。

### (2) profile の `[review]` 節
- D2.2 の値: `profiles/celeris.toml` の high_risk は 7 class（credential・sandbox・store・events・lease・budget・selfdeploy）、invariants は 13 件。`profiles/default.toml` は下位項目の無い `[review]`。
- D2.3・D6.2・D7.3（`check-health.sh` の冒頭注記と `profile.py` の docstring）は dispatch task の子 task review-policy-cli が 1 つの policy の branch で実装し、人の承認の後に operator が main に merge した（D6.6）。試験は `scripts/tests/test_repo_health_review_policy.py` の `test_health_review_` 19 件。
- D6.2 の条件「変更 path が全部 `policy.files`」では、進捗 file と検査器の試験は policy の file に入らない。進捗を書く Celeris の branch は D6.2 の exit 0 にならず、policy の branch は人の承認と operator の merge で通した。

### (3)・(4)・(5) の型
- D3.1・D3.2: `DesignBrief` の形の検証は新しい file `crates/task-core/src/execution_plan/design_brief.rs`（`brief_shape_errors`・`require_design_brief`）。/1・/2 に brief を書いたときのエラーは `PlanValidationError::DesignBrief` でなく既存の `V3FieldNotAllowed { field: "design_brief" }`。brief 専用の大きさの上限は置かず、`max_plan_json_bytes_v3` に数える。試験は `design_brief_` 7 件。
- D4.1: `HealthReportFinding.severity`・`delta`、`HealthFindingsSummary.verdict`、`HealthPolicyState.source` は enum でなく文字列にした。値の正本は repo health の report の schema で、Rust に同じ enum を二重に持たない。wire 型は `crates/task-worker/src/protocol/health.rs`。

### 未実装（決定節にあるが無いもの）
- 設計 review の reviewer prompt（D3.5 2.・D5.4 の `design` の部分）: dispatcher は `ReviewRequest.design` を渡して reviewer run を起こすが、`build_review_prompt` と `change_rules::review` に `ReviewRequest.design` を描く節も、Result 節に `design: {pass, findings}` の形も無い。本物の reviewer は `design` を書かないので、dispatcher は判定を読めない不合格に数え、不合格が `MAX_DESIGN_REVIEW_FAILURES`（2）回続くと計画を採用して `PlanGate` で人に回す。合格はしない（fail closed）が、高 risk の計画は人の承認を経る。流れ全体の試験（`health_review_scenario_high_risk_change`）は偽の reviewer が `review.json` に `design` を直接書くので、この穴を検出しない。
- (7) の撤去のうち D7.1（reviewer prompt の「Commit references in progress files and reports」節）・D7.2（health の無い review の Result 節は旧い形のまま）・D7.4（`PLANNER_CHECK_GUIDANCE` の `CLAUDE.md` と同じ文、`PRODUCTION_HOST_PLANNER_GUIDANCE`）。G2 の計画に U6 の unit が無かった。
- D1.6（Aider に `CLAUDE.md` が届かない）は決定どおり範囲外。

## 付記（2026-11-08、二度目の final review の差し戻しへの方針）
task 01M6TYVJSTYGRPHERB93MM7J59 の二度目の final review（reviewer run 01M6XA5X9W8V2DCAXMQ0631PR9）は受け入れ条件 0・2・3 を不合格にした。理由は次の 5 つ。
1. 設計 review の reviewer prompt に `ReviewRequest.design` を描く節が無く、Result 節にも `design` の形が無い。本物の reviewer は `design` を書かないので、高 risk と unknown（remote workspace を含む）の計画は、失敗の決まった reviewer run と planner の再試行を経て人の承認（`PlanGate`）に回る。上の「付記（G2 の完了時の突き合わせ）」の「未実装」の 1 つ目。
2. project profile（`[review]` 節）が worker の run に届かない（D1.2 の付記）。
3. 設計 review の run は `dispatch_one` の中で `tree_run_limit_hold` より前の `design_review_hold` から起き、木の run・token の上限の判定と `TokenBudgetWatch` を通らない。
4. (7) の D7.1・D7.2・D7.4 が残っているのに、状態が「実装済み」だった。
5. 進捗の「変更」節の件名の一部が、rebase の後の `git log $(git merge-base HEAD main)..HEAD` に無い。

この付記で (a)〜(e) の方針を決める。実装は後続の WorkUnit（design-prompt・retire-legacy・worker-profile・design-budget・flow-tests）が行い、reclose が実装と突き合わせて状態を戻し、5. の件名を直す。この付記を書いた unit（amend-adr）はコードを変えていない。

### (a) 設計 review の reviewer prompt（D3.5 2.・D5.4・D7.2）
- A1 `crates/task-worker/src/change_rules/review.rs` に `design_section(review: Option<&ReviewRequest>) -> String` を足し、`build_review_prompt` から呼ぶ。`ReviewRequest.design` が `Some` のときだけ出す。中身は次のとおり。
  - review の対象は実装でなく、採用前の計画であること。作業ツリーは読み取り専用で読んでよい（owner 地図・既存の実装の確認に使う）。
  - `plan_summary` をそのまま出す。
  - `brief` の全欄: `root_cause`・`owner`・`reuse`・`options`（`name`・`chosen`・`reason`）・`new_state`・`new_branch`・`new_fallback`・`new_port`・`removals`・`invariants`。空の欄は「無し」と出す。`brief` が `None` なら「brief 無し」と書く。
  - `risk` の `level` と `reasons` の全件。
  - 方針: `review_policy` が `Some` なら、`owner_map` の path、`high_risk` の class と paths（正規表現）、`invariants`（名前と根拠の file）を出す。`None` なら「この repo の review 方針を読めなかった」と書き、`risk.reasons` の `review_policy_unavailable:<要約>` を理由として示し、汎用規約と brief だけで判定するよう指示する（(d)）。
  - 判定の観点: 汎用規約の項目（D5.1 の 7 種）に照らして brief と計画を見る。brief が宣言した `new_*` と `removals` が、触る path class と不変条件に対して足りているかを見る。
- A2 Result 節は `result_section` の 1 節だけにする。形は run の種類で変える。
  - 設計 review（`design` が `Some`）: `{"verdicts":[],"design":{"pass":…,"findings":[…]}}`。`ReviewOutput.verdicts` は必須の欄で、設計 review には判定する受け入れ条件が無いので空の配列を書かせる。finding は `HealthFindingOut` と同じ形で、`kind` は 7 種、`severity` は `blocking`・`advisory`。blocking が 1 件でもあれば `pass: false`。`repair` は書かせない（計画の不合格は planner の再試行で直し、局所修復は無い）。違反が無ければ `"findings":[]`。設計 review の prompt には受け入れ条件の判定の節（"Criteria you must judge in this run"）を出さない。
  - final review（`design` が `None`）: `verdicts[]` の各要素に `criterion`・`pass`・`reason` と任意の `repair`（`{"scope":"local","class":…,"hint":…}`）を書く形を常に出す。`health` が `Some` なら、今の `health_verdict_lines` を続ける。
  - D7.2: 旧い文面（`{"verdicts":[{"criterion","pass","reason"}]}` だけを示す文）と、それを固定する試験 `change_rules_review_without_health_keeps_the_legacy_prompt` を消す。代わりの試験は、health の無い final review の Result 節が `repair` を含む形を示すことを確かめる。
- A3 例の JSON は protocol の `ReviewOutput` で読めることを試験で固定する。Result 節の例は placeholder（`<index>`・`<bool>`）を使わず、そのまま JSON として読める値で書く（例: `"criterion":0`、`"pass":false`）。取りうる値の説明は例の外の箇条に書く。試験は描いた Result 節から例の JSON を取り出し、`serde_json::from_str::<ReviewOutput>` で読む。そのうえで、設計 review では `design` が `Some`、health 付きの final review では `health` と `verdicts[0].repair` が `Some` であることを確かめる（`ReviewOutput` は未知の欄を黙って捨てるので、欄名の誤りは `Some` の確認で検出する）。
- A4 dispatcher の読み込みと判定（`health_review/design_verdict.rs`）は変えない。
- 試験名の prefix は `change_rules_review_design_`（`design_section` の全欄、方針が `None` のときの文面）と `change_rules_review_result_example_`（A3）。担当は design-prompt。

### (b) worker の run への project profile
- B1 置き場所: `RunContext.workspace_note: Option<String>` を `RunContext.workspace: Option<RunWorkspaceContext>` に置き換える。`RunWorkspaceContext { note: Option<String>, review_policy: Option<ReviewPolicy> }`（どちらも `serde(default, skip_serializing_if = "Option::is_none")`）。`RunContext` の直下の欄数は 40 のまま変わらない。`ExecutionPlannerContext.review_policy` は消す（この欄へ移す）。2 つはどちらも dispatcher が task の作業場所から決める「この run の作業場所の情報」なので、同じ意味の物としてまとめる。
  - 退けた案: `RunContext` の直下に欄を足す（41 欄になり `size.responsibility-growth` が fail する）。`WorkUnitPromptContext` と `ExecutionPlannerContext` の両方に置く（atomic task の run に届かず、同じ状態を 2 か所に持つ）。`RunContext.profile` に入れる（org の実効 profile で、意味が違う）。`type_fields` の閾値を上げる（policy の file の変更で人の承認が要り、閾値を緩めて通すことになる）。
  - wire の破壊的変更: `workspace_note` の欄は無くなる。互換の読み替えは置かない（[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)）。`docs/protocol/worker-protocol.schema.json` を再生成する。
- B2 方針の読み方: 今の `task_review_policy` の規則（target は review 前の同期と同じ `review_target_ref`、base 側の検査器、`load_review_policy`）を再利用する。LLM は呼ばない。処理を次の 2 つに分ける。
  - dispatcher（tick の中、同期）は target の解決だけを行う。`task_review_policy` を `review_policy_target(task) -> Result<Option<(PathBuf, String)>, String>`（`Ok(None)` = `Absent`、`Err` = `Unavailable` の理由）と読み込みに分け、結果を `RunExtras` に入れる。
  - 読み込み（`git archive` と python。上限は `REVIEW_POLICY_TIMEOUT` の 20 秒）は `run_worker` の非同期部分で `spawn_blocking` を使って行う。scratch target の割り当てと同じ形で、tick を止めない。
  - 読むのは run の起動時に 1 回だけ。WU ごと・tick ごとに LLM で repo を読まない。cache は置かない（新しい状態を足さない）。
  - `planner_review_policy` と、`execution_planner_context` の中の同期の読み込みは消す。planner の run も同じ経路で読む。
- B3 入れる run: task の作業場所に git の worktree がある run のうち、worker の run（atomic task の execute、WU の run、WU の repair・integration_repair）と planner の run（/1・/2・/3、replan）。reviewer の run には入れない。設計 review は `DesignReviewRequest.review_policy`、final review は `HealthRepoInput.owner_rows`・`invariants` で方針を受け取るので、同じ方針を 1 つの prompt に 2 回描かない。CoS chat には入れない（D1.3）。方針を読む repo は今の planner と同じく task の先頭の repo（worker の cwd）。WU の run でも target は task の target（WU の base ではない）。
- B4 `change_rules::repo_section` と `design_brief_section` は `context.workspace` の `review_policy` を読む。`ExecutionPlannerContext` は見ない。
- B5 読めないとき: `Unavailable`（remote workspace・target を解決できない・検査器の失敗・時間切れ）と `Absent` は `review_policy = None` で、prompt に repo 固有の節を出さない。`Unavailable` の理由は今の planner と同じく warn の log に残す。worker・planner の prompt の節は情報で、関門ではない。fail closed の関門は別にある: 計画の採用は `design_gate`（unknown → 設計 review）、候補は `policy_review_gate`（不明 → D6.4 の人の承認）。
- 試験名の prefix は `change_rules_worker_profile_`（atomic の execute・WU の run・planner の run の prompt に repo 固有の節が 1 回出る、reviewer の run と CoS chat に出ない、方針が無いと節が無い）と `health_review_worker_profile_`（dispatcher 側。`RunExtras` に target が入る、remote workspace では入らない）。担当は worker-profile。

### (c) 設計 review の run の予算
- C1 木の run・token の上限: `tree_run_limit_hold` を、gate から `task_core::NextRun` を決める部分と、`NextRun` を受けて判定し `kind: limit` の決定を要求する部分 `tree_limit_hold_for(task, next)` に分ける。`design_review_hold` は、設計 review の run を起こす直前（実行中の run が無く、再試行の上限に達していないとき）に `tree_limit_hold_for(task, NextRun::Worker)` を呼ぶ。true なら run を起こさず `Ok(true)`（hold のまま）を返す。決定の要求は既存の `open_decision` の重複止めで木に 1 件だけ出る。`NextRun` に種類は足さない。設計 review の run は runs 索引の Reviewer の行で、`task_core::tree` の集計では run 数（`runs`）でなく `reviewer_runs` に数え、費用（USD）と token には数える。したがってこの判定は、木の run 数が既に上限に達している・費用が上限を超えた（または費用不明の run がある）ときに設計 review の run を止める。replan の上限は掛けない。人が上限を上げれば、次の tick で設計 review の run が起きる。
- C2 token 予算の watch: `review_spawn.rs` の組み立て（`budget_scope` → `tree_tokens_used` → `TokenBudgetWatch::new`）を `dispatcher/token_budget.rs` の 1 つの関数 `run_watch(store, task, exclude_run: Option<&str>) -> Result<Option<Arc<TokenBudgetWatch>>, StoreError>` に移す。使うのは `review_spawn.rs`（final review）・`dispatch_run.rs`（worker・planner の run）・`health_review/design_gate.rs`（設計 review）の 3 か所。error の扱いは呼び出し側が決める。`dispatch_run.rs` は今のとおり `?`。`review_spawn.rs` は今のとおり warn を出して watch 無しで走る（[ADR 2026-10-20-mlsys-main2-fixes](2026-10-20-mlsys-main2-fixes.md) D1-3 の既存の挙動で、この付記では変えない）。`design_gate.rs` は `?`（run を起こさず、次の tick でやり直す。C1 の判定と同じ扱い）。
- C3 `run_design_reviewer` は watch を受け取る。`review.rs` の reviewer run と同じく、途中で上限に達したら process tree を止めて `BudgetExhausted{Tokens}` で終え、既に上限なら adapter を起こさない。この処理は `review.rs` の reviewer run と共通の関数にし、同じ仕様を二重に実装しない。
- C4 `BudgetExhausted{Tokens}` で終わった設計 review の run は、今の「done で終わらない run」と同じに扱う（`max_reviewer_retries` まで起こし直し、超えたら `DesignReviewed { pass: false, findings: [] }`）。新しい分岐は足さない。予算が尽きた後の起こし直しは adapter を起こさないので、LLM を呼ばず token も使わない。その後の planner の run は既存の token 予算の経路で止まる。代償として、予算の尽きた task では LLM を呼ばない run の行が最大 `max_reviewer_retries + 1` 件増える。
- 試験名の prefix は `health_review_design_budget_`（木の上限を超えると run を起こさず limit の決定が 1 件出る、上限の内なら run が起きる、run に watch が付く、予算が尽きていれば adapter を起こさない）。担当は design-budget。

### (d) remote workspace
- D1 remote workspace の task の方針は今のまま `Unavailable("remote workspace")` で、risk は `unknown`、計画は設計 review に回る（fail closed）。`Absent` には変えない。remote にすれば方針を逃れられる経路を作らないため。
- D2 A1 の節が方針を読めない理由を reviewer に示し、reviewer は汎用規約と brief で判定する。本物の reviewer が `design.pass = true` を書けるので、remote workspace の計画は設計 review の 1 run を経て採用される。人の承認に回るのは、設計 review の不合格が 2 回続いたとき（D3.5 5.）だけになる。
- D3 代償: remote workspace の task は、planner の計画ごとに reviewer の run が 1 回増える。
- 流れ全体の試験（下）の remote workspace の場合で確かめる。

### (e) (7) の撤去
- E1 D7.2 は撤去する（A2）。担当は design-prompt。
- E2 D7.1 は決定を改め、「Commit references in progress files and reports」の節は撤去しない。理由は 2 つ。
  - D7.1 の前提「同じ規則は `CLAUDE.md` にあり全 harness に届く」は agent-platform の repo にしか当たらない。review 前の rebase（[ADR-0118](0118-review-target-sync-and-merge-candidate.md)）はどの repo の task にも起きるが、ほかの repo の `CLAUDE.md` にこの規則は無い。
  - `CLAUDE.md` が書くのは書く側の規則（commit を件名で書く・hash を書かない）だけで、「古い hash を理由に reviewer が不合格にしない」という判定の規則は reviewer prompt のこの節にしか無い。節を消すと、repo 一般に適用するという親 task の原則に反する。
  - 消すのは、書く側の規則を写した 1 文（"Progress files and reports name commits by subject and the base as `git merge-base HEAD main` (`$CELERIS_WU_BASE` for a work unit), and record checks as run on the HEAD at submission."）だけ。ほかの repo の worker はこの規則を受け取らないので、その repo の review ではこの文が事実と合わない。
  - 節の正本は [ADR 2026-10-23-commit-refs-by-subject](2026-10-23-commit-refs-by-subject.md) の D1・D6。旧い仕組みでなく現行の reviewer への指示なので、撤去条件は置かない。1 文の削除の担当は retire-legacy。
- E3 D7.4:
  - 消す: `PLANNER_CHECK_GUIDANCE` の「Records follow ADR-0128 …」の箇条（記録の場所・hash を書かない）と「Whole-workspace tests … run only in the release gate …」の箇条。どちらも agent-platform の `CLAUDE.md` にあり、ほかの repo では当たらない path（`agent-docs/`・`release.sh`・`test-affected.sh`）を指す。
  - 消す: `PRODUCTION_HOST_PLANNER_GUIDANCE`。planner の prompt にも `preamble::fixed_notes` の「本番 host の操作」節が入り、計画では人の決定・人の check を置くことまで書いている。消す前に、/3 の execution plan の prompt にその節がちょうど 1 回あることを試験で確かめる。
  - 撤去しない: 「Do not run CPU-burning load scripts …」の箇条。これは人が決めた共用 host の規則（KB `user/stress-shared-host-rules.md`）で、この host で走る全 repo の run に当たる。今この規則を持つのは agent-platform の `CLAUDE.md` とこの箇条だけで、消すとほかの repo の planner に届かなくなる。host の規則を 1 か所（例: `preamble::fixed_notes`）に移すかは人が決める（task の進捗の提案に書く）。責任者は人（operator）。撤去条件: host の規則を 1 か所に移す変更で、同じ変更でこの箇条を消す。
  - `PLANNER_CHECK_GUIDANCE` のほかの agent-platform 固有の箇条（`check-doc-links.sh`・`corepack pnpm`・`cargo fmt`・`web/`・`docs/` の diff の check）は D7.4 の範囲の外。profile の `[review]` 節などへ移す案を task の進捗の提案に書く。
  - 担当は retire-legacy。
- E4 試験名の prefix は `change_rules_retire_`（D7.1 の節に書く側の規則の文が無く判定の規則は残る、`PLANNER_CHECK_GUIDANCE` に消した 2 箇条が無い、planner の prompt に本番 host の節が 1 回）。

### 流れ全体の試験（flow-tests）
- 今の `health_review_scenario_high_risk_change` は、偽の reviewer が `review.json` に `design` を直接書くので、prompt の穴を検出しなかった。新しい試験の偽の reviewer は `review.json` を自分で組まない。受け取った `RunContext` から本物の `build_review_prompt` で prompt を組み、設計 review の節に brief・risk・方針（または読めない理由）があることを確かめ、prompt の Result 節の例の JSON を書く（判定の値だけを差し替える）。
- 場合は 3 つ: 高 risk（合格で採用される）、remote workspace（unknown → 合格で採用され、人の承認に回らない）、通常の変更（設計 review の run が起きない）。prefix は `health_review_scenario_`。

### 状態
- 状態行を「一部実装（二度目の差し戻しの修正中）」にした。上の後続 unit が終わった後、reclose が決定節とこの付記を実装と突き合わせ、状態を戻す。

## 付記（2026-11-08、差し戻しの修正の完了時の突き合わせ）
後続の WorkUnit（design-prompt・retire-legacy・worker-profile・design-budget・flow-tests）を統合した後の HEAD で、決定節と上の付記「二度目の final review の差し戻しへの方針」（以下、方針の付記）を実装と突き合わせた。各 unit の記録は [design-prompt](../progress/2026-11-07-health-v2-g2/design-prompt.md)・[retire-legacy](../progress/2026-11-07-health-v2-g2/retire-legacy.md)・[worker-profile](../progress/2026-11-07-health-v2-g2/worker-profile.md)・[design-budget](../progress/2026-11-07-health-v2-g2/design-budget.md)・[flow-tests](../progress/2026-11-07-health-v2-g2/flow-tests.md)。どの unit も ADR を編集していないので、方針の付記との違いをここに写す。試験の件数はこの付記を書いた時点の HEAD で `cargo nextest list` が出した関数名を数えた（task-worker の `change_rules_` 25 件、task-dispatch の `health_review_` 80 件）。

### 前の付記の「未実装」と差し戻しの理由の扱い
- 付記「G2 の完了時の突き合わせ」の「未実装」の 1 つ目（設計 review の reviewer prompt、D3.5 2.・D5.4 の `design`）: 済み。design-prompt が `change_rules::review::design_section` と、Result 節の `design` の形を足した（方針の付記の (a)）。
- 同じく 2 つ目（(7) の D7.1・D7.2・D7.4）: 済み。D7.2 は撤去した。D7.1 と D7.4 は方針の付記の (e) E2・E3 で決定を改め、改めた範囲を retire-legacy が撤去した（下の (e)）。
- 同じく 3 つ目（D1.6、Aider に `CLAUDE.md` が届かない）: 決定どおり範囲外のまま。
- 付記「G2 の完了時の突き合わせ」の (1) の D1.2（repo 固有の review 方針の節は /3 の planner の run にだけ出る）: 済み。worker-profile が方針を `RunContext.workspace.review_policy` に移し、worker の run と planner の run の両方に出す（下の (b)）。
- 差し戻しの理由 3.（設計 review の run が木の上限と `TokenBudgetWatch` を通らない）: 済み（下の (c)）。
- 差し戻しの理由 5.（進捗の「変更」節の件名）: reclose が [G2 の進捗](../progress/2026-11-07-health-v2-g2.md) の「変更」節を、`git log --no-merges --format=%s $(git merge-base HEAD main)..HEAD` にある件名だけに直した。

### (a) 設計 review の reviewer prompt（design-prompt）
- 実装の場所は方針の付記のとおり（`change_rules/review.rs` の `design_section`・`result_section`、`claude_code/prompt.rs` の `build_review_prompt` は `review.design` が `Some` のとき `design_section` を出す）。D7.2 の旧い文面、定数 `LEGACY_RESULT`、試験 `change_rules_review_without_health_keeps_the_legacy_prompt` は消した。代わりの試験は `change_rules_review_final_result_without_health_shows_repair`。
- 試験名の prefix: 方針の付記は `change_rules_review_design_` と `change_rules_review_result_example_` を挙げた。実装は unit の受け入れ条件に合わせて `change_rules_design_review_` の 4 件に揃えた。`…_prompt_shows_the_plan_and_the_design_shape`・`…_without_policy_or_brief_explains_why` が前者、`…_result_examples_parse_as_review_output` が後者に当たる。`…_absent_from_final_review` は final review に設計 review の節が出ないことを確かめる。
- A3 の例の JSON: 文字列で書かず、`protocol::ReviewOutput` の値を作って `serde_json::to_string` で書き出す。型と例が食い違わない。
- 設計 review の prompt で出さない節: 方針の付記が挙げた「Criteria you must judge in this run」に加え、worker の summary・evidence・health の節も出さない（設計 review の `ReviewRequest` ではどれも空）。「Acceptance criteria of the task under review」（対象 task の受け入れ条件の一覧）は計画を判定する材料として残した。
- A4: dispatcher の読み込みと判定（`health_review/design_verdict.rs`）は変えていない。

### (b) worker の run への project profile（worker-profile）
- B1〜B5 は方針の付記のとおり実装した。`RunContext.workspace: Option<RunWorkspaceContext { note, review_policy }>` が `RunContext.workspace_note` を置き換えた（[ADR-0039](0039-project-workspace.md) の `workspace_note` は `RunContext.workspace.note` になった）。`ExecutionPlannerContext.review_policy`・`Dispatcher::planner_review_policy`・`Dispatcher::task_review_policy`・`change_rules::review_policy` は消した。`RunContext` の直下の欄は 40 のまま。読み口は `RunContext::workspace_note()` と `RunContext::review_policy()`。schema は再生成した。
- module の置き場所（方針の付記は決めていない）: `health_review/worker_profile.rs` に置き、`dispatcher/run_context.rs` から `#[path]` で子 module として読む（`design_gate.rs` と同じ形）。`dispatcher/` の module の本体の行が閾値を超えているため、また `dispatcher` から `health_review` への依存の辺を足さないため。
- 対話 run（task の会話の宛先がある run）: 方針の付記の B3 は触れていない。実装は対話 run に方針を入れない（`run_workspace` の `with_policy = conversation_addressee.is_none()`）。作業場所の説明（`note`）は従来どおり入る。
- repair・integration_repair の run: 呼び出し口は `dispatch_run.rs` の `run_extras` の 1 か所で、同じ経路を通る。repair の run だけを対象にした試験は無い。
- 試験: `change_rules_worker_profile_` 2 件（task-worker）、`health_review_worker_profile_` 4 件（task-dispatch）。方針の付記の `health_review_worker_profile_` は「`RunExtras` に target が入る」までだったが、実装の試験は adapter が受け取った `RunContext` まで確かめる。
- 残る点: 方針を読む repo は task の先頭の repo だけ（B3 のとおり）。読み込みは run の起動ごとに 1 回で cache は無く、1 回あたりの時間は実測していない。

### (c) 設計 review の run の予算（design-budget）
- C1: `Dispatcher::tree_run_limit_hold` を消し、判定と決定の要求を `Dispatcher::tree_limit_hold_for(task, next)` に、gate から `NextRun` を決める部分を `dispatcher/tree_units.rs` の free fn `next_run(gate)` に分けた（方針の付記は置き場所を決めていない。`Dispatcher` の method 数を増やさないため）。`design_review_hold` は、実行中の run の待ちと再試行の上限の分岐の後、review 方針の読み込みと `DesignReviewRequest` の組み立ての前に判定する（止める場合に方針を読まないため。条件と結果は方針の付記と同じ）。
- C1 の「token の上限」: 木の上限（`TreeLimits`）に token 数の種類は無い。token の使用量から単価で計算した費用の上限 `max_tree_usd` として試験した。
- C2: `dispatcher/token_budget.rs` の `run_watch` が唯一の `TokenBudgetWatch::new` の呼び出し口。`pick_reviewer` は `run_watch` の結果（`Result`）を引数で受け取り、final review は `Err` のとき warn を出して watch 無しで走り、設計 review は `?` の後の `Ok` だけを渡す。方針の付記に無い変更として、`token_budget::exhausted` も `run_watch` を使う形にした（同じ組み立ての重複を消す。挙動は同じ）。
- C3: 共通の関数の名前は `review::run_within_token_budget(adapter, req, run: &ReviewerRun)`（方針の付記は名前を決めていない）。`run_reviewer_inner`（final review）と `run_design_reviewer`（設計 review）の両方が呼ぶ。
- C4: 新しい分岐は足していない。起こし直しのたびに `start_design_review` が `run_watch` を読み直すので、予算が尽きた後の起こし直しは adapter を起こさない。
- 試験: `health_review_design_budget_` 8 件（C1 が 3 件、C2〜C4 が 5 件）。
- 旧い名前の残り: `crates/task-core/src/tree/limits.rs:86` の `TreeLimitKind::is_run_time` の doc comment が `tree_run_limit_hold` を指す。ADR-0079・[2026-10-11-task-wait-reasons](2026-10-11-task-wait-reasons.md)・[2026-10-22-plan-issue-answer-replans](2026-10-22-plan-issue-answer-replans.md) の本文も旧い名前を書く。ADR の本文は当時の記録として変えない。doc comment は design-budget の範囲（`crates/task-dispatch/src/`）と reclose の範囲（文書だけ）の外にあり、直していない。

### (d) remote workspace（flow-tests の remote-flow）
- D1・D2 は方針の付記のとおり成り立つ。試験 `health_review_flow_remote_workspace_plan_is_adopted_after_one_design_review` は、remote workspace の task の計画が risk `unknown`（reasons に `review_policy_unavailable:remote workspace`）で設計 review に回り、本物の prompt（`claude_code::build_prompt`）に方針を読めない理由と brief が出て、1 run の合格で採用されることを確かめる。`PlanApprovalRequested`・`PlanGateDecided`・`DecisionRequested` の event も、人への受信箱の項目も無い。
- 方針の付記との違い: 方針の付記の D1〜D3 は、remote workspace の task が planner の計画を持つことを前提に書いた。今の Complexity Gate は remote workspace の task を最初の dispatch で atomic にする。`crates/task-core/src/execution_gate.rs` の `out_of_scope_rule` の `task.workspace.local_mode() == WorkspaceMode::Shared`（185 行）が、`WorkspaceSpec::local_mode` が `Remote` に `Shared` を返すため、どの remote の task にも当たる（人の明示の compound より先に効く）。そのため remote の task で planner が走るのは、人が採用した計画（`PUT /tasks/{id}/execution-plan`）の WU が `yield.plan_issue` を返した後などの replan だけで、試験もこの経路で planner を起こす。D3 の代償（計画ごとに reviewer の run が 1 回増える）も、この replan のときだけ起きる。
- この付記は今の挙動を記録するだけで、gate を変えるかは決めていない。remote の task の最初の計画を compound にできるよう gate を直す案は [G2 の進捗](../progress/2026-11-07-health-v2-g2.md) の提案に書いた（`crates/task-core/` の変更なので別の task）。同じ条件の 2 つ目の項 `remote_mode() == Shared`（186 行）は 1 つ目の項が `Remote` を全部拾うので結果を変えない。「`Remote` のうち `mode: Shared` だけを対象外にする」意図だったかは未確認。

### (e) (7) の撤去（retire-legacy）
- D7.1（E2）: reviewer prompt の「Commit references in progress files and reports」節から、書く側の規則を写した 1 文だけを消した。見出しと判定の規則は残る。方針の付記との違い: 節の文は `crates/task-worker/src/claude_code/prompt/commit_refs.rs` の定数 `REVIEW_SECTION` に移し、`build_review_prompt` はそれを出す。unit の受け入れ条件（`prompt.rs` に `Commit references in progress files` が無い）は元の D7.1 に合わせて書かれ、E2（節を残す）と食い違うため、文の置き場所を変えて両方を満たした。reviewer prompt に出る文は、消した 1 文を除いて変わらない。節の正本はこの file。
- D7.4（E3）: `PLANNER_CHECK_GUIDANCE` から「Records follow ADR-0128 …」と「Whole-workspace tests … run only in the release gate …」の 2 箇条を消し、本番 host の planner 用の定数 `PRODUCTION_HOST_PLANNER_GUIDANCE` とその呼び出しを消した。planner の prompt には `preamble::fixed_notes` の「本番 host の操作」節がちょうど 1 回入る。方針の付記は確認を /3 の prompt だけと書くが、試験は /2・/3 の両方で確かめる。E4 の「`PLANNER_CHECK_GUIDANCE` に消した 2 箇条が無い」は、定数を直接読まず、`build_prompt` で作った planner の prompt の「check の書き方」節で確かめる。
- 残したもの: 「Do not run CPU-burning load scripts …」の箇条（E3 のとおり。責任者は人、撤去条件は host の規則を 1 か所に移す変更）。`CELERIS_WU_*`・scope check の箇条。`PLANNER_CHECK_GUIDANCE` のほかの agent-platform 固有の箇条（D7.4 の範囲の外。移す案は進捗の提案）。
- 試験: `change_rules_retire_` 3 件。消した文を固定していた `planner_prompt_declares_production_host_changes_as_a_human_procedure` は消し、`change_rules_retire_planner_prompt_has_the_production_host_note_once` に置き換えた。`prompt.rs` は 1,886 行から 1,859 行。

### 流れ全体の試験（flow-tests）
- 3 つの場合を `crates/task-dispatch/src/health_review/flow_tests.rs` に置いた（`dispatcher/tests/mod.rs` から `#[path]` で `mod health_review_flow` として読む）。偽の reviewer `FlowAdapter` は `review.json` を自分で組まず、受け取った `RunRequest` から本物の prompt を組み、設計 review の節の値と Result 節の形を確かめ、Result 節の例の JSON の判定の値だけを差し替えて書く。高 risk（`…_high_risk_plan_is_adopted_after_the_prompt_review`）、通常の変更（`…_normal_change_puts_repo_rules_in_the_work_unit_prompt_only`）、remote workspace（上の (d)）。
- 方針の付記との違い: 試験名の prefix は `health_review_scenario_` でなく `health_review_flow_`（3 件。unit の受け入れ条件の check がこの prefix を数える）。既存の `health_review_scenario_` の 3 件は残した。prompt は `build_review_prompt`（`claude_code::prompt` の private な関数）でなく公開の入口 `task_worker::claude_code::build_prompt` で組む。reviewer の run では `build_prompt` が `build_review_prompt` に分岐する（claude_code adapter の本番の経路と同じ）。
- 検出力の確認: flow-tests は `design_section` から risk の行を手元で外すと高 risk の試験が落ちることを確かめた（commit には含まない）。

### 状態と残る穴
- 状態行を「実装済み」に戻した。決定節（D7.1・D7.4 は方針の付記で改めた形）と方針の付記の (a)〜(e) は、上に書いた違いを除いて実装と合う。
- 決定節・付記の範囲の外に残る穴（実装の未了ではない）:
  - policy の関門: 材料作りの非同期 task の panic では event が残らず関門が掛からない。同期の無い repo と remote workspace の repo は candidate の SHA が決まらず、記録も関門も無い（付記「dispatch の実装」のまま）。
  - remote workspace の task の最初の計画は gate で atomic になる（上の (d)）。
  - `generic.md` の規約 7〜9 に対応する `HealthFindingKind` が無い（付記「G2 の完了時の突き合わせ」の (1)）。
  - `TreeLimitKind::is_run_time` の doc comment の旧い名前（上の (c)）。

## 付記（2026-11-08、三度目の final review の差し戻しの修正）
三度目の final review（reviewer run 01M6Y1RWMFQHQCWV2Z69BGMT9M）は、task の受け入れ条件 3 だけを不合格にした。理由は、この task が置き換えた仕組みの説明が 3 か所に残っていたこと（`CLAUDE.md` の「review では旧い仕組みの残りを不備として指摘する」、`generic.md` の規約 6）。WorkUnit map-rows と limits-doc が並行して直した。変えたのは地図の 2 行と doc comment の 1 行だけで、決定節と付記の仕組みは変えていない。各 unit の記録は [map-rows](../progress/2026-11-07-health-v2-g2/map-rows.md)・[limits-doc](../progress/2026-11-07-health-v2-g2/limits-doc.md)。

### (1) 地図の「健全性の review」の行（map-rows）
- 差し戻し: `docs/architecture-map.md` の行は、もう無い `Dispatcher::planner_review_policy` を入口に挙げ、方針の渡し先を「planner への受け渡し」と書いていた。worker の run への配布、設計 review の予算、`flow_tests`・`design_budget` の試験が載っていなかった。
- 概要: 「planner・worker の run への配布〈`RunContext.workspace.review_policy`〉」に改め、設計 review の run への木の run・費用の上限と token 予算を足した。
- 入口: `planner_review_policy` を消し、付記 (b) の経路（`health_review/worker_profile.rs` の `Dispatcher::run_workspace`・`Dispatcher::review_policy_target`、run の起動で `RunWorkspace::into_context` が `load_review_policy` を 1 回呼ぶ）と、付記 (c) の経路（`design_review_hold` → `tree_limit_hold_for`、`start_design_review` → `token_budget::run_watch`、adapter は `review::run_within_token_budget` の下で走る）を書いた。名前と場所は map-rows が実装を読んで確かめた。
- ADR の欄に方針の付記の (b)・(c) を足した。関連試験に `health_review/worker_profile/tests.rs`・`health_review/flow_tests.rs`・`dispatcher/tests/health_review_design_budget.rs` を足した。不変条件に付記 (b)（方針は run の起動で 1 回だけ読み、読めなければ入れない）と付記 (c)（設計 review の run も木の上限と token 予算の下で走る）の 2 文を足した。

### (2) 『共通 agent 指示と配布』の汎用規約と review 方針の配布の行（map-rows）
- 差し戻し: 節に汎用規約の正本 `change_rules/generic.md` とその配布（`prompt_header`）の行が無かった。
- 必須規則の行の直後に 1 行を足した（地図は 213 行から 214 行）。入口は `crates/task-worker/src/change_rules.rs`（正本 `change_rules/generic.md`、`GENERIC_RULES`・`section`・`repo_section`）、組み立て口の `claude_code/prompt.rs` の `prompt_header`、方針の配布の `health_review/worker_profile.rs`（`Dispatcher::run_workspace`・`RunWorkspace::into_context`）。関連試験は `change_rules_generic_`・`change_rules_worker_profile_`・`health_review_worker_profile_` の file。不変条件は D1 と付記 (b)（正本は 1 file で `CLAUDE.md` と互いに写さない、方針は base 側から読み、読めなければ節を出さない）。

### (3) `limits.rs` の doc comment の旧い名前（limits-doc）
- 差し戻し: `crates/task-core/src/tree/limits.rs:86` の `TreeLimitKind::is_run_time` の doc comment が、design-budget が置き換えた `tree_run_limit_hold` を指していた。
- `Dispatcher::tree_limit_hold_for` に直した。task-core から task-dispatch の非公開の method へは rustdoc の link を作れないので、backtick の名前で書いた。コードの振る舞いは変えていない。`crates/` に `tree_run_limit_hold` は 0 件。
- 付記「差し戻しの修正の完了時の突き合わせ」の (c) の「旧い名前の残り」と「状態と残る穴」の最後の項目のうち、doc comment は直した。ADR の本文（この ADR の方針の付記、ADR-0079、[2026-10-11-task-wait-reasons](2026-10-11-task-wait-reasons.md)、[2026-10-22-plan-issue-answer-replans](2026-10-22-plan-issue-answer-replans.md)）と過去の進捗 file の旧い名前は、当時の記録として残す。

### 状態と残る穴
- 状態行は実装済みのまま、三度目の差し戻しの修正を含むことを書き足した。
- `check-architecture-map.py` は表の path の実在と列数しか見ないので、(1)・(2) を検出しなかった。地図の行の `<型>::<fn>` の名前を入口の file と照らす検査の案は、map-rows の提案と [G2 の進捗](../progress/2026-11-07-health-v2-g2.md) の提案にある。
- 付記「差し戻しの修正の完了時の突き合わせ」の「状態と残る穴」のうち、doc comment の項目を除く 3 項目は残る。地図は 214 行で、文書の 200 行の目安を超えている。
