---
title: アーキ v4 C prompts — worker・reviewer・planner の実際の prompt に新しい依存辺・risk の根拠・安全条件を渡す
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
completed: 2026-11-18
updated: 2026-11-18
---
# prompts（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の D4 と「## 付記（2026-11-18、計画 v3 の決定）」。
基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。main は merge していない。
commit は件名で挙げる: `arch-v4 C prompts: reviewer・worker・planner の prompt に risk の根拠（path・class・owner 地図の行・不変条件）・finding の symbol と rationale・境界と依存の規則を渡す`、
`arch-v4 C prompts: 試験の方針の根拠の file と owner 地図を doc の link と見なされない名前にする`、この進捗の commit。

## ADR D4 との違い（closeout が ADR の付記に写す）
- D4 は「地図の行の探索を純粋関数として `task_core::design_review` に置く（path かその親 dir を含む最初の行、長い接頭辞から探す）」と書いた。
  実装は新しい探索関数を作らず、final review の health の材料が既に使う `task_dispatch::health_review::input::owner_rows` を再利用した。
  描画も既存の `change_rules::review::owner_lines`。探索の規則は `owner_rows` のまま（行の `…` の中の path が変更 path そのもの・親 dir・
  `foo.rs` の module dir を含む行を、地図の順に最大 40 件）。D4 の「最初の 1 行」「長い接頭辞から」の規則は使わない。
- 設計 review（採用前の計画）の地図の行は、wire の任意の欄 `DesignReviewRequest.owner_rows`（既存の `OwnerRow` の配列。serde default、空なら JSON に出さない）で渡す。
  dispatcher（`health_review/design_gate.rs` の `design_review_hold`）が、判定に使った方針（`definition_sha256` が `DesignReviewRequested.policy_sha256` と同じ）の
  repo の base（target）の版の `owner_map` を `git show` で読み、risk の理由の path（`path_class:<class>:<path>`）を先に、計画の write path を後に
  `owner_rows` へ渡す（`input::requested_policy`）。final review の差分の設計の節（`diff_design`）では `owner_rows` を空にし、worker が同じ request の
  `HealthRepoInput.owner_rows` を使う（同じ行を 2 か所に持たない）。
- D4 の「`HealthReportFinding.key`」は付記 (ii) のとおり作らず、review-gate が写した `symbol`・`rationale` を描く（鍵の書式は組み直さない）。

## 実装前 brief（7 項目）
1. 根本原因: reviewer の prompt は risk の理由を語のまま並べるだけで、path の owner・class の不変条件・design が要る理由が無い。health の finding は
   `rule_id`・severity・delta・path しか出ず、B の新しい組（`symbol`）と `rationale` が reviewer に見えない。worker の repo 固有の規約は旧い規則
   （「計画は brief が要り設計 review の対象」だけ）で、class の不変条件と依存の規則が無い。修復の run の前回の判定（`prior_review`）にも新しい辺が無い。
2. 責務 owner: reviewer の節は `task-worker/src/change_rules/review.rs`（`risk_lines`・`findings_lines`・`design_section`）、worker と planner の節は
   `change_rules.rs`（`repo_section`）、設計 review の材料は dispatcher の `health_review/design_gate.rs`、修復の reason は `health_review/gate.rs` の `health_repairs`。
3. 既存の再利用先: `input::owner_rows`・`show_at`・`path_pair`、`owner_lines`、`policy::load_review_policy`、`PathClass.invariants`（main に入った欄）、
   `HealthReportFinding.symbol`・`rationale`（review-gate）、`prior_review` の経路（`health_repairs` の reason → `ReviewVerdict` → `prior_review_from_events`）。
4. 選択肢: (a) `task_core::design_review` に地図の探索関数を足す（D4 の原文）→ `owner_rows` と同じ仕様の二重実装になるので不採用。(b) `owner_rows` を再利用し
   wire の任意の欄で渡す（採用。objective の指定）。地図の読み込みの置き場所は (c) dispatcher の module に行を足す → `dispatcher` の module（size.responsibility-growth
   の existing の単位）が増えるので不採用、(d) 方針の読み込みの base を `repo_review_base` に切り出し、方針と地図の読み込みを `health_review` の
   `input::requested_policy`・`policy::base_review_policy` に置く（採用。dispatcher の module の本体の行は 482 → 473 で減った）。
5. 新しい状態・分岐・fallback・port: 状態・分岐・fallback は無し。port は wire の任意の欄 `DesignReviewRequest.owner_rows` 1 つ（省略した JSON は従来どおり読める）。
   `HealthFailure.symbols`（dispatcher の内部の型）。
6. 撤去対象: `Dispatcher::repo_review_policies`（`repo_review_base` と `plan_review_policies` に置き換え。method の数は同じ）、`repo_section` の旧い規則の文
   「この class の path を変える計画は設計 brief が要り、設計 review の対象になる」、理由を語だけで出す `risk_lines` の行、旧い形の行を前提にした試験の期待値 3 件
   （`change_rules_review_with_health_shows_every_field`・`change_rules_design_review_prompt_shows_the_plan_and_the_design_shape`・
   `prompt_final_design_keeps_criteria_evidence_health_and_adds_the_diff_design_section` の 1 行ずつ。新しい形に置き換えた）。planner の brief の節の旧い規則の文は
   risk-rule が既に消していた（残りは無い。`design_brief_section` の「`new_*` の宣言によらず High」は現行の規則）。
7. 不変条件と試験: 下の表。sandbox の class の path・policy の file（`scripts/dev/repo-health/`）・安全不変条件の code は変えていない。

## 変更
- `crates/task-worker/src/protocol/design_review.rs`: `DesignReviewRequest.owner_rows: Vec<OwnerRow>`（`#[serde(default, skip_serializing_if = "Vec::is_empty")]`）。
- `crates/task-worker/src/change_rules/review.rs`:
  - `risk_lines(risk, policy, rows)`: level の後に「design の判定が要る理由」（`high`: 高 risk の class の path に合う。`new_*` の宣言の有無によらない・`new_fallback`。
    `unknown`: 方針・正規表現・差分を読めず決められない）。理由ごとに `reason_detail`: `path_class:<class>:<path>` は
    「path・class・owner 地図の行（`row.path == path` の行。無ければ「この path の行は無い」）・class の不変条件（`PathClass.invariants`、根拠の file は方針の
    `invariants`）」を 1 行、`invariant:<class>:<名前>` は根拠の file。`<repo>:` の付いた複数 repo の理由も同じ。
  - `design_section`: 行は採用前の計画では `design.owner_rows`、差分では `health.repos[].owner_rows`。採用前の計画では `owner_lines` で一覧も出す。
  - `findings_lines`: new・worsened の finding に `fingerprint`・`symbol`（あれば）と `rationale` の行。
- `crates/task-worker/src/change_rules.rs`（`repo_section`）: class の規則の文を「class の path を変える計画・差分は `new_*` の宣言によらず設計 review の対象（既存の判定の
  条件を 1 行変えるだけでも同じ）。planner の計画は brief が要る。計画に書かずに触れても final review で `design` を求められる」に置き換え、class ごとに
  `（不変条件: …）`（`PathClass.invariants` から出す。文言は写さない）、依存の規則 `DEPENDENCY_RULE`（既存の循環の中に新しい組 `<src> -> <dst>` を足すと node の数が
  変わらなくても `deps.cycle` が fail。health の材料と前回の判定に `symbol` で出る）を足した。
- `crates/task-dispatch/src/health_review/design_gate.rs`: `repo_review_policies` の closure を method `repo_review_base`（repo の dir と target、読めなければ方針の状態）に
  し、`plan_review_policies` は `base_review_policy` で読む。`design_review_hold` は `input::requested_policy` で方針と地図の行を得て `DesignReviewRequest.owner_rows` に入れる。
- `crates/task-dispatch/src/health_review/input.rs`: `requested_policy`（方針を `definition_sha256` で選び、その版の地図を `show_at` で読み `owner_rows` を呼ぶ）。`diff_design` は `owner_rows: Vec::new()`。
- `crates/task-dispatch/src/health_review/policy.rs`: `base_review_policy`（bundled の検査器と `REVIEW_POLICY_TIMEOUT` で `load_review_policy`。dispatcher と `requested_policy` の 2 か所で使う）。
- `crates/task-dispatch/src/health_review/gate.rs`: `HealthFailure.symbols`（finding の evidence の `symbol`）。`health_repairs` の reason を
  `health:<fp>（evidence: <paths>、symbol: <src> -> <dst>）` にした（`; ` を使わないので `repair_buckets` の区切りは同じ）。この reason が `ReviewVerdict` を経て
  修復の run の `prior_review`（「Previous attempt's review result」）に載る（既存の経路。新しい経路は無い）。`check_item.rs` の module doc の形も直した。
- 生成 schema `docs/protocol/worker-protocol.schema.json`（`UPDATE_SCHEMA=1 cargo nextest run -p task-worker -E 'test(/schema/)'`）と `docs/protocol/worker-protocol.md` の設計 review の節。
  `docs/api/v1/`・`web/api/generated/schema.json` は `DesignReviewRequest` を含まず変わらない。

## codex・pi・acp が同じ節を使うことの確認
- `codex.rs` は `crate::claude_code::build_prompt`、`acp.rs`・`pi.rs` は `build_prompt_with_skill_list`（CoS chat 以外は `build_prompt` に skill 一覧を足すだけ）を呼ぶ
  （review-wire の確認と同じ。この WU で変更なし）。`build_prompt` は全 run kind で `prompt_header` → `change_rules::section`・`repo_section` を通り、
  review では `build_review_prompt` → `health_section`・`design_section` を通る。harness ごとの写しは無いので、関数の追加の呼び出しは要らなかった。

## 試験（守る不変条件と、既存の試験で検出できない理由は各試験の注釈）
| 試験 | 確かめること |
|---|---|
| `prompt_boundary_design_review_shows_reason_owner_row_and_invariants` | `build_prompt` の設計 review の prompt に、理由 `path_class:authz:<path>` の 1 行（path・class・`owner_rows` の地図の行・`api-principal-from-token` と根拠の file）、`invariant:` の根拠、design が要る理由（`high`・`unknown`）、地図の行の一覧。行の無い path は「この path の行は無い」 |
| `prompt_boundary_final_review_shows_finding_symbol_rationale_and_owner_row` | final review の health の節に `fingerprint`・`symbol`（`<src> -> <dst>`）・`rationale`、差分の設計の節の理由の行に `HealthRepoInput.owner_rows` の行 |
| `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule` | worker（execute）と planner の prompt の repo 固有の節に、宣言によらない設計 review の規則・final review で `design` を求められること・class の不変条件・`deps.cycle` の依存の規則。3,000 文字の objective が末尾まで載る |
| `health_enforce_entry_repairs_carry_symbols`（task-dispatch） | `health_repairs` の reason が `symbol: src/a -> src/c` を運び、`; ` を含まない（`prior_review` の経路） |
| `health_review_design_gate` の既存の試験（stub の方針に `owner_map` を足した） | 実際の dispatcher の設計 review の request の `owner_rows` が base の版の地図の行（`path` は理由の path）になる |

objective の全文の配信は、既存の `health_review/reduction_flow_tests.rs`（WU の `task_objective` の全文。loc-reduction-v3 B）と上の 3 件目で確かめた。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 範囲の check（計画の式。`sh "$CELERIS_WU_SCOPE_PATHS"` の path に範囲外の path が無い）: 範囲外 0 件。変更 path は上の「変更」と試験・生成 schema・文書・この進捗。
- `cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)'`: 3 passed。`fn prompt_boundary_` は 3 件。
- 設計 review の `owner_rows` の check（`DesignReviewRequest` の `pub owner_rows:` と `design_gate.rs` の `owner_rows`）: 両方ある。
- `cargo nextest run -p task-dispatch -E 'test(/design/)'`: 28 passed。`-E 'test(/health_enforce_entry|review_risk_|design|health_review_input|scenario|policy_review|verdict/)'`: 109 passed。
- `bash scripts/dev/check-health.sh --out …`: exit 0。verdict pass、new 0・worsened 0・existing 697。必須 rule は `policy.changed` を含め全部 `pass`
  （`deps.cycle`・`size.responsibility-growth` も pass）。1 回目は試験の literal（`docs/…md` の架空の path）を `doc.links` が壊れた link と数えて fail。名前を変えて直した。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（753 秒）。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker（`--list`）。
  nextest 6,097 passed・21 skipped、doc test は ok、script bundle の SKIP 0 件。
- 全体の試験と release gate は走らせていない（release gate だけで走る）。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`。doc comment を含む）
- production Rust: +261 −80（純増 181）。task-worker +124 −11（`change_rules/review.rs` +95 −6、`change_rules.rs` +23 −4、`protocol/design_review.rs` +6 −1）、
  task-dispatch +137 −69（`design_gate.rs` +57 −64、`input.rs` +42、`gate.rs` +24 −4、`policy.rs` +13、`check_item.rs` +1 −1）。
- 試験: +229 −5。生成 schema +7。文書 +4 −1。
- size.responsibility-growth: `dispatcher` の module（existing）の本体の行は `design_gate.rs` で 482 → 473（空行・注釈・use を除く数え方）。`Dispatcher` の method の数は同じ
  （`repo_review_policies` を `repo_review_base` に置き換え）。`claude_code/prompt.rs`（1,898 行）は変えていない。

## 未解決・提案
- 規模: C 全体の production の Rust の純増は risk-rule 29・review-wire 94・review-gate 370・この WU 181 で計約 674 行になり、ADR の目安 400 行を超えた。
  closeout か H が、超過を受け入れるか縮めるかを判断する（提案）。
- `owner_rows` は行ごとに「最初に当たった変更 path」を `path` に持つ。差分（final review）では変更 path の順が diff の順なので、理由の path が別の path の行に
  覆われると、理由の行に「この path の行は無い」と出る（行自体は health の節の一覧にある）。採用前の計画では理由の path を先に渡すので起きない。
- `cases` の leaf: ケース 1 の「prompt に地図の行と `api-principal-from-token`」は `prompt_boundary_design_review_shows_reason_owner_row_and_invariants` の形、
  ケース 4 の「reviewer の prompt に `<src> -> <dst>`」は `prompt_boundary_final_review_shows_finding_symbol_rationale_and_owner_row` の形で確かめられる。

## 本番への移行
- config・DB の変更は無い。release が昇格した後の daemon と worker から効く（昇格は人だけ）。
- `DesignReviewRequest.owner_rows` は任意の欄で、旧い worker は読み飛ばす（`deny_unknown_fields` を持たない）。旧い daemon の request（欄なし）は空として読む。
- 昇格の後、health の修復の reason に `symbol:` が付く。昇格の前の reason（`symbol` なし）も `repair_buckets` は同じに読む。
