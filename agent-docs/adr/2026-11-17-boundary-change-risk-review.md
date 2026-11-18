---
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
---
# ADR 2026-11-17: 安全境界の意味を変える改修を、新しい仕組みの宣言によらず設計 review に回す（risk の規則・class の補完・final review の差分 risk）

- 日付: 2026-11-17
- 状態: Accepted（決定だけ。この ADR を書いた WorkUnit `adr` は code・profile・scripts を変えていない。実装は同じ task の WorkUnit
  `profile`（policy の唯一の leaf）・`risk-rule`・`review-gate`・`prompts`、実証は `cases`、計測は `metrics` が行う）
- 付記先（closeout が末尾に付記を足す）: [ADR 2026-11-07 health review integration](2026-11-07-health-review-integration.md) の D2.2（class）・D3.3（risk の規則）・D3.5（設計 review）・I8（policy の人の承認）。
- 関係: [ADR 2026-11-17 deps.cycle の新しい組と論理 module](2026-11-17-repo-health-scc-edges-logical-modules.md)（アーキ v4 B。finding の鍵 `cycle-edge:<src>-><dst>`）、
  [ADR 2026-10-27 API の主体](2026-10-27-api-principals.md)、[ADR-0080](0080-browser-phase2-policy-broker-approval.md)、
  [ADR 2026-11-04 project controller](2026-11-04-project-controller.md) D12、[2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- task: 01M7MZTKB29VF5TX7FCQ15XEMW（アーキ v4 C。依頼キー celeris-architecture-scalability-v4-20261117、親 01M7MZT8KX547464YYS76QNP4G、依存 B 01M7MZTKAZQY72EX2R4RQ1WVWF）。
  基準は main `bc665168c9caf1898adc4c611404bbee056daf9a`。before の値と計測の規則は A の baseline（run の artifacts の `baseline.md`）§3.6・§6・§7・§8。

## 背景

- `task_core::design_review::assess_risk`（`crates/task-core/src/design_review.rs:214-248`）が `High` を返すのは、write path が高 risk の class に合い
  brief が `new_state`・`new_branch`・`new_fallback`・`new_port` のどれかを宣言したときと、`new_fallback` を宣言したときだけ。
  既存の認可・lease・transaction・event の判定の条件を 1 行変える改修は、新しい仕組みを宣言しなければ `Low`（`:248`）、brief が無ければ
  `Low`（`brief_absent_human_plan`、`:245-246`）になる（baseline §3.6）。
- 判定の入力は計画の write path（`execution_plan::plan_write_paths`。planner の宣言）だけで、実際の差分は見ない。宣言しなかった path や
  rename・削除で高 risk の file に触れても、採用前の判定にも final review にも出ない。
- profile `scripts/dev/repo-health/profiles/celeris.toml:176-268` の 7 class に、次の境界の file が合わない（baseline §3.6・§8）:
  `crates/task-api/src/knowledge.rs`（添付の pin の IMMEDIATE tx、`:874-878`）、`crates/task-api/src/cos/operations.rs`（CoS の業務操作の認可と監査）、
  `crates/task-dispatch/src/dispatcher/worker_finish.rs`（worker run の終了処理）、`crates/task-worker/src/browser_policy.rs`（Browser の grant と policy）。
- reviewer の prompt（`crates/task-worker/src/change_rules/review.rs:256` の `risk_lines`）は理由の語をそのまま出し、owner 地図の行と class ごとの不変条件を出さない。
  health の new finding の写し（`crates/task-worker/src/protocol/health.rs:106-117` の `HealthReportFinding`）は `rule_id` と `fingerprint`（hash）だけで、
  B の鍵 `cycle-edge:<src>-><dst>` を持たない。

## 実装前 brief（7 項目）

### 1. 根本原因
- risk の規則が「高 risk の path」と「新しい仕組みの宣言」の積になっている（`design_review.rs:229-243`）。境界の意味は既存の分岐の条件を変えるだけで変わるので、
  宣言の有無は境界の変更の有無を表さない。症状は、認可・lease・tx・event の条件の変更が `Low` で採用され、設計 review を経ずに入ること。
- 判定が計画の宣言だけを見て、実際の差分を見ない。症状は、宣言の漏れ・rename・削除で高 risk の file を変えても設計 review が起きないこと。
- profile の class が境界の file を網羅していない（上の 4 file と、D2 の表の file）。

### 2. 責務 owner
- risk の規則: `crates/task-core/src/design_review.rs`（`assess_risk`・`high_risk_classes`。純粋関数。I4）。
- 計画の採用前の判定: `crates/task-dispatch/src/health_review/design.rs`（`merge_risk`）・`design_gate.rs`。
- final review の材料と判定: `crates/task-dispatch/src/health_review/input.rs`（G2 の材料。`HealthReportFinding` を作る `:717`）・`verdict.rs`。
- class の定義と検証: `scripts/dev/repo-health/profiles/celeris.toml` の `[review]`、`scripts/dev/repo-health/health/profile.py`（`_validate_review`・`review_policy`）、
  `scripts/dev/repo-health/schema/review-policy-v1.schema.json`。Rust 側の型は `design_review.rs` の `PathClass`。
- prompt: `crates/task-worker/src/change_rules.rs`（`repo_section`。worker と planner）・`change_rules/review.rs`（reviewer）。

### 3. 既存の再利用先
- `high_risk_classes`・`assess_risk`・`merge_risk`・`RiskAssessment`（理由の語）・`RiskLevel::needs_design_review`。
- 設計 review の型と描画: `ReviewRequest.design`（`DesignReviewRequest`）、`ReviewOutput.design`（`DesignVerdictOut`）、`design_section`・`design_verdict_lines`。
  final review で設計の判定を求めるときも同じ型・同じ読み方を使う（新しい reviewer の run・新しい verdict の型を足さない）。
- base 側の方針の読み込み `health_review::policy::load_review_policy`（候補の profile を読まない。I3）。
- policy の変更の関門 `health_review/policy_review.rs`（I8。reviewer の承認の後も人の `Approval` の子が要る）。
- B の finding（`deps.cycle` の鍵 `cycle-edge:<src>-><dst>`、delta `new` は fail 級）と G2 の材料の `HealthReportFinding`。
- 試験: `crates/task-core/src/design_review/tests.rs`、`health_review` の `*_tests.rs`、`change_rules` の試験、B の `scripts/tests/test_repo_health_scc_edges.py`。

### 4. 選択肢
| 案 | 内容 | 採否と理由 |
|---|---|---|
| A. 規則の積を和にし、final review で実差分にも当てる | path が class に合えば宣言によらず `High`。final review で実差分（rename の両側・削除）に class を当て、採用した計画の設計 review が覆わない class が残れば reviewer に既存の `design` の判定を求める | **採用**。owner は既存の 2 か所（`assess_risk` と final review の材料）だけで、型・run・描画を再利用できる。宣言の漏れ・rename を実差分で閉じる |
| B. 宣言の欄を足す（`changes_boundary` など） | brief に境界の変更の宣言を足し、宣言で `High` | 不採用。宣言に頼る点が根本原因と同じで、宣言しない改修を止められない |
| C. 差分の意味の解析（認可の条件の AST の比較など） | 条件式の変化を検出 | 不採用。新しい解析基盤になり（依頼の禁止）、言語ごとに要る |
| D. 実差分の検査を別の reviewer run（第 2 の設計 review）にする | final review の前にもう 1 run | 不採用。run と費用が増え、新しい待ちの状態が要る。final review の reviewer に同じ run で `design` を書かせれば足りる |

### 5. 新しい状態・分岐・fallback・port
- 状態: 無し（新しい表・列・event kind・メモリ上の状態を足さない。判定の記録は既存の event の欄に載せる。D3）。
- 分岐: final review の材料に「覆われない class があるとき `ReviewRequest` に設計の節を付ける」分岐を 1 つ（D3）。
- fallback: 無し。読めない・取れないときは fail-closed（D3・D5 の 6）。
- port: (a) profile の `[[review.high_risk]]` に任意の `invariants`（不変条件の名前の配列）と、review 方針の JSON の `high_risk[]` の同名の欄（D2・D6）。
  (b) `HealthReportFinding.key`（report の finding の鍵の写し。D4）。(c) final review の `ReviewRequest` に、覆われない class の risk を載せる欄 1 つ（D3。`design` と同じ `DesignReviewRequest` を使う）。
  どれも既存の wire の型に欄を足すだけで、新しい API・CLI・config 欄は無い。

### 6. 撤去対象
- `assess_risk` の積の規則と理由の語 `path_class:<class>+<欄>`（`design_review.rs:229-237`）。代わりは `path_class:<class>:<path>`（D1）。
- 「高 risk の class に合い brief が無い → `Low`（`brief_absent_human_plan`）」の規則（`:245-246`）。語 `brief_absent_human_plan` は `High` の補足の理由として残す（D1）。
- `DesignBrief` と `RiskAssessment` の doc comment の旧い規則の記述（`design_review.rs:45-46`・`:146-147`・`:205-213`）、`DesignBrief.new_fallback` の doc の「宣言すれば path に依らず高 risk」は残す。
- 旧い規則を前提にした試験（`design_review/tests.rs` の「高 risk の path でも宣言が無ければ Low」の類）。代わりの試験は D5 の `boundary_case_1_`・`2_`。
- 新旧切替の flag・互換分岐は置かない。

### 7. 不変条件と確かめる試験
- risk の判定は LLM も I/O も使わない純粋関数のまま（I4）: `design_review/tests.rs`。
- 方針は base 側の版だけから読む。候補が自分の判定の条件を変えられない（I3）: `boundary_case_5_`。
- policy の file の変更は reviewer の承認の後も人の承認が要る（I8）: `boundary_case_5_`、既存の `policy_review` の試験。
- 読めない・取れない・判定の無い review は合格にしない（fail-closed。J4）: `boundary_case_6_`。
- 高 risk の class に合わない普通の変更は今までどおり `Low` で、設計 review も final review の設計の節も起きない: `boundary_case_2_` と replay（web/・docs/ だけの commit は 0 件）。
- 安全不変条件（SQLite 正本・append-only events・lease/atomicity・replay・ACL・credential・CoS 監査・project controller の安全停止・selfdeploy の本番権限・Rust/TS canary）の
  code は変えない（変えるのは判定と prompt だけ）。既存の試験は `test-affected` で確かめる。

## 決定

### D1. risk の規則: 高 risk の class に合う write path は宣言によらず High
`assess_risk(policy, write_paths, brief)` の規則を、上から順に次にする。

1. 方針が `Absent` → `None`（`review_policy_absent`）。`Unavailable`・正規表現を読めない → `Unknown`（`review_policy_unavailable:<要約>`）。今のまま。
2. write path が高 risk の class に合う → `High`。理由は class と合った path の組ごとに `path_class:<class>:<path>`（class は方針の順、path は辞書順）。
   brief の `new_*` の宣言は level を変えない。宣言した欄は理由の後ろに `declared:<欄>` として足す（reviewer が読む材料。level には効かない）。
3. brief が `new_fallback` を宣言 → path に依らず `High`（`new_fallback`）。今のまま。
4. それ以外 → `Low`（理由なし）。

brief の無い計画（planner 以外の計画）の扱い: 規則 2 に合えば `High` にし、理由に `brief_absent_human_plan` を足す。設計 review の reviewer は brief 無しで計画の要約を
判定する（`design_section` は brief が無いとき「brief 無し」と出す。今の描画のまま）。planner の計画の brief の必須（`execution_plan::require_design_brief`、
`brief_required_classes`）は今のまま。今の code で `assess_risk` を呼ぶのは planner の計画の採用（`design_gate`）と `assess_plan_risk` だけで、
planner 以外の計画が採用前の判定を通る経路があるかは未確認（leaf `risk-rule` が呼び出し元を確かめる）。採用前の判定を通らない計画の変更は D3 が final review で覆う。

### D2. class の補完と class ごとの不変条件
既存の 7 class は残し、次を足す（正規表現は repo 相対 path に当てる。Rust の `regex` と python の `re` の共通部分だけを使い、先読みを使わない）。
各 path は base（main）の追跡 file に当たることを確かめた（replay の道具の検査）。

| class | 足す path（正規表現） | 境界（読んだ file と根拠） | `invariants` |
|---|---|---|---|
| `authz`（新） | `^crates/task-api/src/principal\.rs$`、`^crates/task-api/src/middleware\.rs$`、`^crates/task-api/src/handlers/authority\.rs$`、`^crates/task-core/src/authority\.rs$`、`^crates/task-api/src/cos/operations\.rs$` | 主体の決定（`principal.rs`。bearer → 主体）、全要求の認証と `require_admin`・`require_active`（`middleware.rs:246-260`）、人の許可範囲の API と規則（`handlers/authority.rs`・`task_core::authority`）、CoS の業務操作の認可と監査（`cos/operations.rs` の `OperationAudit`・`Applied`） | `api-principal-from-token`（新。`agent-docs/adr/2026-10-27-api-principals.md`）、`cos-audit-same-tx`（新。`agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` の `OperationAudit::apply` の 1 transaction） |
| `transaction`（新） | `^crates/task-api/src/knowledge\.rs$`、`^crates/task-ops/src/lifecycle\.rs$` | store の外で接続と tx を持つ file（`knowledge.rs:874-878` の IMMEDIATE tx、`lifecycle.rs:110-116` の raw `Connection` の closure）。store の中の `write_tx`（`store/mod.rs:1014`）は `store` の class が覆う | `task-state-in-sqlite`、`lifecycle-human-epoch-same-tx`（新。`agent-docs/adr/2026-11-04-project-controller.md` の I6（CAS）と D12） |
| `event-apply`（新） | `^crates/task-core/src/transition\.rs$`、`^crates/task-ops/src/replay\.rs$` | 状態機械の遷移（地図の「Task 型・状態機械」行）と events からの再構成（task-ops::replay）。追記と行の遷移（`store/events.rs`・`store/transition.rs`）は `events`・`store` が覆う | `events-append-only`、`task-state-in-sqlite` |
| `browser`（新） | `^crates/task-worker/src/browser_policy\.rs$`、`^crates/task-worker/src/browser_credential\.rs$`、`^crates/task-worker/src/browser_cdp_credential_ipc\.rs$`、`^crates/task-worker/src/browser_launcher_run\.rs$`、`^crates/task-api/src/browser_credentials\.rs$` | grant と policy の決定（ADR-0080 D1）、supervisor だけの credential の区画（D2/D3）、launcher の credential の受け渡し、保存した credential の管理 API（`require_admin`） | `credential-stays-in-broker` |
| `worker-finish`（新） | `^crates/task-dispatch/src/dispatcher/worker_finish\.rs$` | worker run の終了処理（同じ transaction の組と quota の解放）。run の終わりの判断（`run_settlement`）は `lease`・`budget` が覆う | `completion-not-self-reported`、`lease-single-holder`、`budget-unknown-cost-stops` |
| `review-gate`（新） | `^crates/task-core/src/design_review\.rs$`、`^crates/task-dispatch/src/health_review/(design\|design_gate\|design_verdict\|policy\|policy_review\|gate)\.rs$` | risk の判定と設計 review・policy の関門・health の入口（判定の条件そのもの。自分の判定を弱める変更を設計 review に回す） | `completion-not-self-reported`、`policy-change-needs-human`（新。`agent-docs/adr/2026-11-07-health-review-integration.md` I8）、`no-llm-in-dispatcher` |
| `selfdeploy`（既存に足す） | `^crates/celeris/src/instance\.rs$` | active の引き継ぎ（地図の self-deploy 行 `docs/architecture-map.md:160` の entry point。今は `lease` だけが覆う） | `unpromoted-release-no-takeover` |

既存の class にも `invariants` を付ける: `credential` → `credential-stays-in-broker`、`sandbox` → `worker-db-read-only`・`worker-no-subagents`、
`store` → `task-state-in-sqlite`・`no-data-loss`、`events` → `events-append-only`、`lease` → `lease-single-holder`、`budget` → `budget-unknown-cost-stops`、
`selfdeploy` → `unpromoted-release-no-takeover`。

- `invariants` は任意の欄（他の repo の profile は書かなくてよい）。名前は同じ profile の `[review.invariants]` の key でなければ profile の読み込みエラー（`_validate_review`）。
  新しい名前 4 つ（`api-principal-from-token`・`cos-audit-same-tx`・`lifecycle-human-epoch-same-tx`・`policy-change-needs-human`）を `[review.invariants]` に足す。
  根拠の file の節が名前の意味を述べているかは leaf `profile` が読んで確かめ、合わなければ根拠の file を差し替える。
- 足さないもの（replay の数と理由）: `crates/task-dispatch/src/dispatcher/work_units.rs`（区間の非 merge 492 件中 10 件・2.0%）。WU の gate と並列の
  scheduling で、状態の変更（終了・lease）は `run_settlement`・`worker_finish.rs` を通るので、それらの class が覆う。H の確認で抜けが見つかれば足す。
- `store` の class は狭めない。区間で `store` に合う非 merge commit は 60 件。store の試験の file だけを変えた commit は 7 件（1.4%）で、狭めても減る数が小さい。
  `store/mod.rs`・`events`・`transition`・migrations だけに狭めると、領域別の SQL（`store/<領域>.rs`。task-state-in-sqlite の本体）が外れる。
  試験の file を正規表現で外すには先読みが要り、Rust の `regex` で書けない。

### D3. final review: 実際の差分に class を当て、覆われない class に設計の判定を求める（fail-closed）
- 材料: final review の材料を作るとき（`health_review::input`）、`git diff --name-status --find-renames <base>..<head>` の path を集める。rename・copy は両側、
  削除は消した path、追加・変更はその path。これを base 側の方針（`load_review_policy`。候補の profile は読まない。I3）で D1 の `assess_risk` に当てる
  （brief は渡さない。同じ関数で、2 つ目の判定を書かない）。
- 覆う class: task に採用した計画があり、その `plan_sha256` の `DesignReviewed` が合格なら、その `DesignReviewRequested.risk` の理由に出た class を「覆った」とする。
  それ以外（採用した計画が無い・設計 review を経ていない・不合格）は覆った class 無し。
- 判定: 差分の class から覆った class を除いて残りがあれば、final review の `ReviewRequest` に設計の節（`DesignReviewRequest`。要約は差分の stat、risk は残った class の
  理由、方針は base 側のもの）を付け、reviewer に同じ run で `review.json` の `design`（既存の `DesignVerdictOut`）を書かせる。`design` が無い・読めない・`pass = false` なら
  substantive の不合格（局所修復なし）。prompt の文は「対象は採用前の計画」でなく「対象は差分」にする（見出しと前置きだけを分け、brief・risk・方針・観点の行は共有する）。
- fail-closed: base 側の方針を読めない（`Unknown`）・差分を取れない・正規表現を読めない → 全差分を覆われないものとして扱い、設計の判定を求める。読めない理由は risk の理由に残す。
  方針が `Absent` の repo は今までどおり何もしない。
- 記録: risk の理由（class・path・不変条件）と設計の判定は、既存の final review の記録（`HealthReviewInputBuilt` の input と review の verdict の event）に載せる。
  新しい event kind は足さない。既存の event の欄に載らない場合は leaf `review-gate` が replan を申告する（範囲の外に回避策を置かない）。
- 効く時期: D3 は D3 を含む release が昇格した後の daemon で効く（昇格は人だけ）。この task 自身の final review は今の daemon で走る。

### D4. prompt に渡すもの
- reviewer（設計 review と D3 の設計の節。`risk_lines`）: 理由 `path_class:<class>:<path>` ごとに、path、owner 地図の行（base の `owner_map` の file で、path か
  その親の dir を文字列として含む最初の行の行番号と第 1 列。長い接頭辞から探し、無ければ「地図に行なし」）、class の `invariants`（名前と根拠の file）を 1 行で出す。
  地図の行の探索は表の意味を解釈しない（arch_map の検査器の判定を写さない）文字列の検索で、純粋関数として `task_core::design_review` に置く。dispatcher が base の版の
  地図を読んで渡す。
- reviewer（final review の health の節）: `HealthReportFinding` に report の finding の `key` を写す欄を足し、new・worsened の finding を `rule_id`・`key`・`fingerprint` で出す
  （B の `cycle-edge:<src>-><dst>` が reviewer に見える）。
- worker と planner（`repo_section`）: class ごとに path と `invariants` を出し、規則「高 risk の class の path を変える計画・差分は、新しい仕組みの宣言によらず設計 review の対象。
  計画に書かずに触れても final review で設計の判定を求められる」を出す。前回の review の health の new finding のうち `deps.cycle` の鍵は、既存の再試行・修復の前置き
  （review の feedback）に出す。
- objective と受け入れ条件が切れずに worker と reviewer の prompt に入ること（1,500 字で切れた不具合の再発防止）を、leaf `prompts` が試験で確かめる（既存の試験があればそれを挙げる）。

### D5. 6 ケースと失敗注入（WorkUnit `cases`）
試験名は `boundary_case_<n>_<要約>`。実際の判定経路（`assess_risk`・`merge_risk`・final review の材料と verdict の読み）を通す。

| # | 試験 | 入力 | 期待 |
|---|---|---|---|
| 1 | `boundary_case_1_authz_one_line_is_high` | `crates/task-api/src/middleware.rs` の `require_admin` の条件を 1 行変える計画（brief は `new_*` 無し）と同じ差分 | 採用前 `High`（`path_class:authz:crates/task-api/src/middleware.rs`）、設計 review を要求。prompt に地図の行と `api-principal-from-token` |
| 2 | `boundary_case_2_equivalent_plain_change_is_low` | 同じ大きさの変更を class に合わない file（`web/src/` の画面 1 file と task-api の handler 1 file）に | `Low`・理由なし、設計 review も final review の設計の節も無し |
| 3 | `boundary_case_3_move_cannot_escape` | 一時 git repo で `principal.rs` を別の dir へ rename（git mv）、および削除と別名の追加 | final review の差分の class に `authz`（rename の元・消した path）。設計の判定が無ければ不合格 |
| 4 | `boundary_case_4_worsening_dependency_blocks` | 既存の循環の内部に新しい組を足した report（B の `cycle-edge:` の new finding、error） | health の判定が fail、reviewer の prompt に `cycle-edge:<src>-><dst>` |
| 5 | `boundary_case_5_policy_self_approval_impossible` | 候補が profile の `[review]` から `authz` を消し、同じ候補で middleware を変える | 判定は base 側の方針で `High` のまま。policy.changed で reviewer の承認の後も人の `Approval` の子を待つ |
| 6 | `boundary_case_6_fail_closed` | 方針を読めない・正規表現を読めない・差分を取れない・覆われない class があるのに `design` の無い review.json | `Unknown` → 設計 review、final review は不合格 |

失敗注入: 各ケースについて、守る規則を 1 か所戻した版（例: 1 は積の規則に戻す、3 は rename の元を集めない、6 は `design` の欠落を合格にする）で該当の試験が落ちることを
確かめ、戻した差分と落ちた試験名を進捗に書く（戻した版は commit しない）。過剰に止めないことは、replay の件数（下）と、ケース 2 で示す。

### D6. policy の変更の扱いと現行の daemon との互換
- profile と検査器の変更（`celeris.toml` の `[review]`、`health/profile.py` の `_validate_review`・`review_policy`、`schema/review-policy-v1.schema.json` の `high_risk[]` の項目、
  profile の試験）は leaf `profile` だけが行う。leaf `profile` は policy.changed の別 review（人の差分承認・operator の main への hand merge）を通す。後の leaf は
  `scripts/dev/repo-health/` を変えない。leaf `profile` の health の check は「`policy.changed` 以外の必須 rule が pass か not_applicable」、merge の後の範囲の check は件名で探した
  merge commit とその第 1 親の差で見る（B の replan の教訓）。
- 現行の daemon（main の release）が新しい欄を読めるかを code で確かめた:
  - `PathClass`（`crates/task-core/src/design_review.rs:39-45`）は `Deserialize` を derive し、`#[serde(deny_unknown_fields)]` を付けていない。serde は未知の欄を捨てるので、
    `high_risk[]` の項目の `invariants` は読み飛ばされる。`ReviewPolicy`（`:22-37`）も同じ。
  - JSON を読む `ReviewPolicyOut`（`crates/task-dispatch/src/health_review/policy.rs:47-55`）は `deny_unknown_fields` を持つが、効くのは top level の欄だけ。
    `high_risk` は `Vec<PathClass>` なので項目の欄は上のとおり捨てられる。
  - 結論: 項目に欄を足すだけなら現行の daemon は読める（方針は `Present` のまま。旧い積の規則で判定する）。**top level に欄を足すと `Unavailable` になり、全計画が `Unknown`
    で設計 review に回る**ので、leaf `profile` は top level の欄を足さない。schema の id（`/1`）は変えない（項目の任意の欄の追加で、読む側は上のとおり許す）。
  - Rust の新しい `PathClass.invariants` は `#[serde(default)]` にする（`invariants` を出さない他の repo の base の検査器・bundled の検査器の JSON も読む）。互換分岐ではなく、欄が任意であることの表現。
- merge の直後（Rust の leaf が release に入る前）の挙動: 新しい class の path を触る planner の計画は brief が必須になる（`brief_required_classes`）。level は旧い規則のまま。

## replay（設計 review の対象の件数）

使い捨ての道具（`$TMPDIR` の python。commit しない。写しは task の run の artifacts の `replay/`）で、main の first-parent の commit（`--since=2026-10-28T00:00Z`、
baseline §6 の 7・§7 C10 と同じ区間と `--no-renames`。rename は両側の path が出る）に、base（`bc665168` の `celeris.toml`）の 7 class と D2 の class を当てた。
REV は baseline と同じ `bc665168`（main の現在の head でも数は同じ。commit 数だけ 499・665 に増え、増えた commit はどの class にも合わない）。

| 集合 | commit 数 | base の class に合う | D2 の class に合う | D2 で新たに合う |
|---|---|---|---|---|
| first-parent の非 merge | 492 | 80（16.3%） | 94（19.1%） | 14 |
| first-parent 全部（merge は第 1 親との差） | 653 | 110 | 129 | 19 |
| うち web/・docs/ だけを変えた commit（非 merge・全部） | 10・11 | 0 | 0 | 0 |
| うち agent-docs/ だけを変えた commit（非 merge・全部） | 206・254 | 0 | 0 | 0 |

class ごと（非 merge）: base は store 60・credential 12・sandbox 8・budget 8・events 7・selfdeploy 5・lease 4。D2 の新しい class は authz 13・review-gate 10・worker-finish 6・
transaction 4・event-apply 2・selfdeploy の追加 1・browser 0。候補で足さなかった `work_units.rs` は 10。store の内訳: store の試験の file を含む 35、
store の試験の file だけ 7、`store/mod.rs` 17、migrations 13、`store/events.rs`・`store/transition.rs` 3。

読み方: 旧い規則で `High` になったのは「80 件のうち brief が `new_*` を宣言したもの」で、宣言は commit から分からない（件数は未確認）。D1 の後は class に合う全部が
`High` になるので、設計 review の上限の目安は非 merge で 94 件／区間（約 3 週間）。commit は task より細かいので、task の数ではこれより少ない。費用は WorkUnit `metrics` が
本番の設計 review の run の記録で見積もる（A の baseline の final review の reviewer run は wall 中央値 1,137.5 s・$2.047）。普通の UI・文書の変更（web/・docs/・agent-docs/ だけ）は
0 件で、過剰に止めない。

## 規模の目安と成果に数えないもの
- 新しい module・trait・framework・reviewer の run の種類を足さない。production の Rust の純増の目安は 400 行以内（D1 は書き換え、D3 は材料に 1 分岐と既存の型の再利用、
  D4 は描画の行と純粋関数 1 つ）。超える見込みなら leaf が replan を申告する（健全性 v2 は約 2 万行の純増で失敗した）。
- class の追加・prompt の行の追加は「判定が変わったこと」を 6 ケースで示して初めて成果に数える。移動・改名・抽象の追加だけを成果にしない。

## 後続 WorkUnit への引き継ぎ
- `profile`: D2 の表と D6。policy の唯一の leaf。
- `risk-rule`: D1。撤去対象（brief 6）を同じ変更で消す。呼び出し元の確認（D1 の未確認）。
- `review-gate`: D3。既存の event の欄に記録できなければ replan。
- `prompts`: D4。objective の全文の試験。
- `cases`: D5 と失敗注入。
- `metrics`: 共通受入条件 (1)〜(6) の before/after（A の baseline §7 の同じコマンド）と、この replay の道具での after の件数・設計 review の回数と費用。

## 付記（2026-11-18、計画 v3 の決定）

計画 v2 の設計 review（2026-11-18）の blocking 2 件を受けて、計画 v3 で次を決めた。WorkUnit `review-wire` が実装の前にここへ書く。
D3・D4 の記述と食い違うところは、この付記が優先する。

- (i) port (c)（§5）の欄は `ReviewRequest.diff_design`（`Option<Box<DesignReviewRequest>>`。serde の default で、`None` なら JSON に出さない）にする。
  final review の run だけが持つ。`ReviewRequest.design` は今までどおり採用前の計画の設計 review の run（dispatcher の `Completion::DesignReview` の経路）だけが持つ。
  run の種類の正本は dispatcher の 1 つで、worker は `design` と `diff_design` のどちらがあるかで描画を分けるだけ（worker が run の種類を推測しない）。
  worker の final review の prompt は、`diff_design` があれば人の決定・決定的 check・evidence・health の節を保ったまま設計の節（見出しは「差分」）を足し、
  `review.json` には criterion ごとの `verdicts`（任意の `repair`）・health があれば `health`・`design`（既存の `DesignVerdictOut`）を同じ object に書かせる。
  `design_section` は見出しと前置きだけを「採用前の計画」と「差分」で分け、brief・risk・方針・観点の行は共有する。
- (ii) port (b)（§5、D4 の reviewer の health の節）の `HealthReportFinding.key` は作らない。report v1 の既存の欄（finding の最初の evidence の `symbol`
  と finding の `rationale`）を `HealthReportFinding` の任意の欄に写す（WorkUnit `review-gate`）。鍵の書式 `cycle-edge:<src>-><dst>` を Rust で組み直さない。
  policy の file（`scripts/dev/repo-health/`）は変えない。
- (iii) final review で覆う範囲は class でなく (class, path) の組にする。採用した計画の合格した設計 review の risk の理由 `path_class:<class>:<path>` に、
  同じ class で同じ path（または `/` で終わる dir の接頭辞）がある差分の path だけを覆う（class 単位では、同じ class の別の file の変更を素通しするため）。
- (iv) `ReviewerPassReused`（同じ差分に対する reviewer の合格の再利用）は変えない。D3 を含む release が昇格する前に出た合格が昇格の後に再利用される限界は、
  進捗の「本番への移行」節に書く。
- (v) reviewer run の無い review（policy_only）で、覆われない (class, path) の組が残れば不合格にする（fail-closed。理由に class と path を残す）。
