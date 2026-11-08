---
title: 健全性v2 G2 design-prompt — reviewer prompt に設計 review の節と Result 節の design の形を足し、旧い Result 節を消す
tasks: [01M6XD44YN834TDGV1FC9AX3MS]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 design-prompt — reviewer prompt に設計 review の節と Result 節の design の形を足し、旧い Result 節を消す

完了日: 2026-11-08

WorkUnit `design-section`（task 01M6XD44YN834TDGV1FC9AX3MS）。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の D3.5 2.・D5.4・D7.2 と、付記「二度目の final review の差し戻しへの方針」の (a) A1〜A4 を実装した。ADR は編集していない。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
- commit の件名: 「G2 design-prompt: reviewer prompt に設計 review の節と Result 節の design の形を足し、旧い Result 節を消す」、「G2 design-prompt: 進捗に変更・検査の結果・付記との違いを書く」。
- `crates/task-worker/src/change_rules/review.rs`
  - `design_section(review: Option<&ReviewRequest>) -> String` を足した（A1）。`ReviewRequest.design` が `Some` のときだけ節「## 設計 review（採用前の計画）」を出す。中身: 対象は採用前の計画で作業ツリーは読み取り専用であること、`plan_summary`、`brief` の全欄（`root_cause`・`owner`・`reuse`・`options` の `name`/`chosen`/`reason`・`new_state`・`new_branch`・`new_fallback`・`new_port`・`removals`・`invariants`。空は「無し」、`brief` が `None` なら「brief 無し」）、`risk` の `level` と `reasons` の全件、`review_policy` の `owner_map`・`high_risk` の class と paths・`invariants`。`review_policy` が `None` なら「この repo の review 方針を読めなかった」と `risk.reasons` の `review_policy_unavailable:…` を理由として示し、汎用規約と brief だけで判定させる（(d)）。最後に判定の観点（汎用規約の 7 項目と種別の名前、`new_*`・`removals` の足りなさ、owner での修正）を出す。
  - `result_section` を 1 節にした（A2）。設計 review では `{"verdicts":[],"design":{"pass":false,"findings":[{kind,severity,path,reason}]}}` の例と 7 種の `kind`・`severity`・`path` の説明を出し、`repair` は書かせない。final review では `verdicts[]` の各要素に `criterion`・`pass`・`reason` と任意の `repair`（`{"scope":"local","class":…,"hint":…}`）を書く形を常に出し、`health` が `Some` なら同じ例の object に `health` を足して `health_verdict_lines` の箇条を続ける。
  - 例の JSON は `protocol::ReviewOutput` の値を作って `serde_json::to_string` で書き出す（A3）。placeholder（`<index>`・`<bool>`）は使わない。種別は `HealthFindingKind::ALL` から選ぶ（review.rs に種別の名前を書かない既存の試験を守る）。final review の例の `criterion` は判定する criterion の先頭。health の例の `policy` は policy の file を変えた repo があるときだけ入る。
  - D7.2: 旧い文面（`{"verdicts":[{"criterion":<index>,"pass":<bool>,"reason":"..."}]}` だけを示す文）を消した。
- `crates/task-worker/src/claude_code/prompt.rs`: `build_review_prompt` は `review.design` が `Some` のとき `design_section` を出し、「Worker's self-reported summary」「Criteria you must judge in this run」「Evidence self-reported by the worker」と health の節を出さない（match の guard を 1 つ足しただけ。描画は review.rs）。prompt.rs は 1,881 行から 1,886 行（上限 2,000 行）。
- `crates/task-worker/src/change_rules/review/tests.rs`
  - 定数 `LEGACY_RESULT` と試験 `change_rules_review_without_health_keeps_the_legacy_prompt` を消した。
  - 代わりの試験 `change_rules_review_final_result_without_health_shows_repair`: health の無い final review と review context の無い run の Result 節が `repair` を含む例と説明を示し、旧い文も placeholder も無く、例が `ReviewOutput` で読めて `verdicts[0].repair` が `Some` であることを確かめる。
  - `change_rules_review_result_section_asks_for_the_health_verdict` を新しい形に合わせた（旧い文面の接頭を確かめる部分を消し、例に placeholder が無いこと・`policy` の例が policy の変更時だけ出ることを確かめる。種別の出現数に例の 1 件を足した）。
  - 設計 review の試験 4 件（prefix `change_rules_design_review_`）:
    - `change_rules_design_review_prompt_shows_the_plan_and_the_design_shape`: `build_prompt` で作った設計 review の prompt に、節・計画の要約・brief の全欄・risk・review 方針・判定の観点（7 種）と Result 節の `design` の形があり、受け入れ条件の判定・summary・evidence・health の節が無い。
    - `change_rules_design_review_result_examples_parse_as_review_output`: Result 節の例の JSON を `ReviewOutput` で読み、設計 review では `design` が `Some`（verdicts は空、repair 無し）、health 付きの final review では `health` と `verdicts[0].repair` が `Some`。
    - `change_rules_design_review_absent_from_final_review`: design の無い final review（health の有無に依らない）と review context の無い run の prompt に設計 review の節と `design` の形が出ない。
    - `change_rules_design_review_without_policy_or_brief_explains_why`: brief 無し・方針を読めない（`review_policy_unavailable:remote workspace`）・理由の無い risk・空の欄の文面。
  - `codex/tests.rs`・`claude_code/tests.rs` の Result の文面の assert（`Write your verdicts to …/review.json`）は残した文と合うので変えていない。

## 付記との違い
- 試験名の prefix: 付記 (a) は `change_rules_review_design_`（`design_section` の全欄・方針が `None` の文面）と `change_rules_review_result_example_`（A3）を挙げた。この unit の objective と task の受け入れ条件 2 は `change_rules_design_review_` を 3 件以上と決めているので、設計 review の試験と A3 の例の読み込みの試験は `change_rules_design_review_` に揃えた（`…_prompt_shows_the_plan_and_the_design_shape`・`…_without_policy_or_brief_explains_why` が付記の `change_rules_review_design_` に、`…_result_examples_parse_as_review_output` が `change_rules_review_result_example_` に当たる）。health の無い final review の代わりの試験は `change_rules_review_` の prefix に置いた。
- 例の JSON の作り方: 付記 A3 は「そのまま JSON として読める値で書く」とだけ書く。この実装は文字列で書かず、`ReviewOutput` の値を serde で書き出す。型と例が食い違えない。
- 設計 review の prompt では、付記が挙げた「Criteria you must judge in this run」に加えて、worker の summary・evidence・health の節も出さない（設計 review の `ReviewRequest` ではどれも空）。「Acceptance criteria of the task under review」の節（対象 task の受け入れ条件の一覧）は計画を判定する材料として残した。
- A4: dispatcher の読み込みと判定（`health_review/design_verdict.rs`）は変えていない。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。先頭の check-health は `verdict=pass … new=0 worsened=0`。nextest は 5,715 件合格・22 件 skip。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo nextest list -p task-worker -E 'test(/change_rules_design_review_/)' --message-format oneline | grep -c change_rules_design_review_`: 4。
- `! grep -rn 'change_rules_review_without_health_keeps_the_legacy_prompt' crates/`: exit 0。`LEGACY_RESULT` も crates/ に無い。
- `cargo nextest run -p task-worker -E 'test(/change_rules_/)'`: 20 件合格。

## 未解決事項
- 無し。dispatcher 側が `design` を読んで採用する流れ全体の試験は unit flow-tests が受け持つ。

## 提案
- 無し。
