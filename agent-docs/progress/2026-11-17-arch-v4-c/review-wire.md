---
title: アーキ v4 C review-wire — final review の設計の節を ReviewRequest.diff_design で渡し、worker の prompt が verdicts・health・evidence を保つ
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
completed: 2026-11-18
updated: 2026-11-18
---
# review-wire（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の §5 port (c)・D3 と、
この WorkUnit が書いた「## 付記（2026-11-18、計画 v3 の決定）」(i)〜(v)。基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。
commit は件名で挙げる: `arch-v4 C review-wire: final review の設計の節を ReviewRequest.diff_design で渡し、prompt が verdicts・health・evidence を保って design も求める`、
`arch-v4 C review-wire: 進捗に検査の結果を書く`。

## 変更
- ADR の付記（受け入れ条件 3）: (i) port (c) の欄は `ReviewRequest.diff_design`（final review だけ）、(ii) `HealthReportFinding.key` は作らず
  report v1 の `symbol`・`rationale` を写す、(iii) 覆う範囲は (class, path) の組、(iv) `ReviewerPassReused` は変えない、(v) policy_only に覆われない組があれば不合格。
- `crates/task-worker/src/protocol.rs`（受け入れ条件 0）: `ReviewRequest.diff_design: Option<Box<DesignReviewRequest>>`
  （`#[serde(default, skip_serializing_if = "Option::is_none")]`）。`design` は採用前の計画の設計 review の run（`Completion::DesignReview` の経路。
  dispatcher の `health_review/design_gate.rs` だけが `Some` を入れる）だけが持つ。
- `crates/task-worker/src/change_rules/review.rs`（受け入れ条件 1）:
  - `design_section` は `design`（採用前の計画）か `diff_design`（差分）を見て、見出し（`DESIGN_HEADING`・新しい `DIFF_DESIGN_HEADING`）と前置きと
    要約の見出し（「計画の要約」・「差分の要約」）だけを分ける。brief・risk・方針・観点の行は同じ関数で共有する。run の種類は dispatcher が欄で決め、
    worker は欄を見るだけ（private の `DesignTarget` と `design_target`）。
  - `result_section` の final review の分岐は、`diff_design` があれば例の JSON に `design` を足し、`verdicts`・任意の `repair`・health があれば `health` の行を保ったまま
    「同じ object に `verdicts` と並べて `design` も書く」の行を足す。`design_verdict_lines` は採用前の計画と差分で前置き・語（不合格の対象・path の対象・申し送り先・
    `repair` を書かない理由）だけを分け、他の行を共有する。採用前の計画の文面は 1 文字も変えていない。
- `crates/task-worker/src/claude_code/prompt.rs`（受け入れ条件 1）: `build_review_prompt` の final review の分岐で、health の節の後に `diff_design` があれば
  `design_section` を足す。人の決定・決定的 check・policy 違反・summary・criteria・evidence・health の節は今のまま。`design` の分岐は変えていない（コメントだけ）。
- `crates/task-dispatch/src/review.rs`: `into_request` の struct literal に `diff_design: None`（値は WorkUnit `review-gate` が入れる）。
- 旧い doc comment 4 か所の書き換え（受け入れ条件 2）:
  1. `task-dispatch/src/review.rs` の `design: None` の注記 → 「採用前の計画の設計 review の run（`Completion::DesignReview` の経路）だけが持つ。ここは final review なので常に `None`」。
  2. `protocol/design_review.rs` の module doc と `DesignReviewRequest` の doc・`plan_summary` の doc → `design`（計画の要約）と `diff_design`（差分の stat・覆われない組の理由）の 2 か所で使う。
  3. `change_rules/review.rs` の module doc の `design_section`・`result_section` の行 → `diff_design` と、見出し・前置きだけを分けること。
  4. `protocol.rs` の `ReviewOutput.design` の doc → `design`（`verdicts` は空）と `diff_design`（`verdicts` と並べる）の review が書く。`ReviewRequest.design` の doc も final review では常に `None` と書いた。
- `docs/protocol/worker-protocol.md`: §3 の表と §10.2 に `diff_design?` と節「final review の差分の設計の節」を足した。
- 生成 schema: `UPDATE_SCHEMA=1 cargo nextest run -p task-worker -E 'test(/schema/)'` で `docs/protocol/worker-protocol.schema.json` を再生成
  （`ReviewRequest.diff_design` と description）。`docs/api/v1/`・`web/api/generated/` は `ReviewRequest`・`DesignReviewRequest` を含まず、変わらない。

## codex・acp・pi が同じ prompt を使うことの確認（受け入れ条件 5）
- `crates/task-worker/src/codex.rs:27` は `use crate::claude_code::{build_prompt, work_dir_note}`、`:723` で `build_prompt(&req.task, &req.context, …)`。
- `crates/task-worker/src/acp.rs:48` は `use crate::claude_code::build_prompt_with_skill_list`、`:849`・`:1034` で呼ぶ。
- `crates/task-worker/src/pi.rs:18` は同じ use、`:199` で呼ぶ（CoS chat run 以外）。
- `build_prompt_with_skill_list`（`claude_code/prompt.rs:99`）は CoS chat run 以外では `build_prompt` に skill の一覧を足すだけで、`build_prompt` は
  `TaskKind::Review` で `build_review_prompt` を呼ぶ。よって 4 つの harness の reviewer run はどれも同じ `build_review_prompt` と `result_section` を通り、
  `diff_design` の節と `design` の形を受け取る（harness ごとの分岐は無い）。

## 試験（`crates/task-worker/src/change_rules/review/tests.rs`。注釈に守る不変条件と既存の試験で検出できない理由を書いた）
| 試験 | 入力 | 期待 |
|---|---|---|
| `prompt_final_design_keeps_criteria_evidence_health_and_adds_the_diff_design_section` | 人の決定・summary・criteria・evidence・health・`diff_design` 付きの final review を `build_prompt` で描く | 人の決定 → summary → criteria → evidence → health → 差分の設計の節 → Result の順に各 1 回。設計の節に差分の要約・brief・risk の理由・方針の class・観点。「採用前の計画」の見出しと「受け入れ条件ごとの判定はしない」は出ない |
| `prompt_final_design_result_example_has_verdicts_and_design` | 同じ request（health 有り・無し） | Result 節の例の JSON を `ReviewOutput` で読むと `verdicts`（criterion 1、`repair` 付き）と `design`（`repair` 無し）があり、health 有りなら `health` も。「`verdicts` と並べて `design` も書く」の行があり、「`verdicts` は空の配列」は無い |
| `prompt_final_design_section_shares_every_line_after_the_preface` | 同じ `DesignReviewRequest` を `design` と `diff_design` に入れて `design_section` | 要約の見出しの後の文字列（brief・risk・方針・観点）が同一。見出しだけが違う |

既存の設計 review の試験（`change_rules_design_review_*` 5 件、`protocol::design_review` の試験）は変えずに通る。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 範囲の check（計画の式そのまま）: exit 0（出力なし）。変更 path は ADR・`task-dispatch/src/review.rs`・`task-worker` の `change_rules/review.rs`・
  `change_rules/review/tests.rs`・`claude_code/prompt.rs`・`protocol.rs`・`protocol/design_review.rs`・`docs/protocol/worker-protocol.md`・
  `docs/protocol/worker-protocol.schema.json`・この進捗。
- `cargo nextest run -p task-worker -E 'test(/prompt_final_design_|design_review/)'`: 11 passed。`fn prompt_final_design_` は 3 件。
- wire・dispatcher・prompt の `diff_design` の grep（計画の check 3）: exit 0。
- `bash scripts/dev/check-health.sh --out …`: exit 0（143 秒）。必須 rule は `policy.changed` を含め全部 `pass`（policy の file は変えていない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: 下の「test-affected」に書く。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）
- production Rust: +161 −67（純増 94。ADR の目安 400 行以内。内訳 `change_rules/review.rs` +124 −52、`protocol.rs` +13 −5、
  `protocol/design_review.rs` +12 −7、`claude_code/prompt.rs` +7 −2、`task-dispatch/src/review.rs` +5 −1）。うち doc comment の書き換えが多い。
- 試験: +166 −2。生成 schema: +15 −4。文書（worker-protocol.md）: +16 −3、ADR: +20。
- 新しい module・trait・reviewer の run の種類・verdict の型は足していない（既存の `DesignReviewRequest`・`DesignVerdictOut`・`design_section`・`result_section` を再利用）。

## 未解決・後続への引き継ぎ
- `review-gate`: `task-dispatch/src/review.rs` の `into_request` の `diff_design: None` を、覆われない (class, path) の組があるとき `Some` にする。
  reviewer の prompt と `review.json` の形はこの WorkUnit で済んでいる（`ReviewOutput.design` を読んで合否に使うのは `review-gate`）。
- `prompts`: `risk_lines` は理由を 1 行ずつ出すだけ（D4 の owner 地図の行・不変条件の根拠の file は未対応）。`design_section` の共有部分を変えれば
  採用前の計画と差分の両方に効く。
- 本番への移行: この WorkUnit だけでは挙動は変わらない（dispatcher は `diff_design` を常に `None` で送る）。新しい欄は追加だけで、旧い worker は
  未知の欄として読み飛ばす（`ReviewRequest` は `deny_unknown_fields` を持たない）。
