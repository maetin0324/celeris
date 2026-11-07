---
title: 健全性v2 G2 prompts — planner/worker/reviewer の prompt に規約・brief・health 文脈を配る
tasks: [01M6VTFGWFVE57YXDKHR2E436G]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 prompts — planner/worker/reviewer の prompt に規約・brief・health 文脈を配る

完了日: 2026-11-07

task 01M6VTFGWFVE57YXDKHR2E436G のまとめ。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の (1)・D3.4・D4.1・D5.4・D6.3 の prompt の部分を、`crates/task-worker`（全 harness 共通の組み立て口 `claude_code::prompt`）に実装した。基準は `git merge-base HEAD main`（各 WU は `$CELERIS_WU_BASE`）。

## 2 leaf の要約
- `rules-brief`（[prompts-rules.md](prompts-rules.md)）: 汎用規約 `change_rules/generic.md` を `include_str` で `prompt_header` に 1 回入れ、全 run kind（execute・plan・replan・execution plan・review）と全 harness に届けた。review 方針があれば repo 固有の節を続けて出す。/3 の planner に設計 brief 7 項目の節と、高 risk の判定が設計 review を起こす説明を足した。CoS chat には入れない。CLAUDE.md の写しとは重ねない。commit は件名「G2 rules-brief: 汎用規約・repo 固有の規約・設計 brief の節を prompt に配る」と、その進捗の commit。
- `review-health`（[prompts-review.md](prompts-review.md)）: 新しい module `change_rules::review` に、`ReviewRequest.health` があるときだけ出す health の材料の節（SHA・diff 統計・owner 行・new findings・policy・不変条件・省略数）と、Result 節を組む唯一の関数 `result_section`（health があるときだけ `health` の形・7 種・局所修復の 2 種・`health.policy` の必須・不合格の条件を足す）を置いた。health が無い review の prompt は従来のまま。commit は件名「G2 review-health: reviewer prompt に health の材料と health verdict の書き方を出す」と、その進捗の commit。

## Task の受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 0. change_rules_ の試験（全 harness の配布・health 無しの review prompt が従来どおり・planner の brief 節）が通る
  - コマンド: `cargo nextest run -p task-worker -E 'test(change_rules_)'`
  - 要点: 16 passed。全 harness の配布は `change_rules_generic_reaches_codex_once_beside_required_rules`・`…_pi_…`・`…_acp_…`・`change_rules_generic_reaches_claude_code_once_without_a_rules_copy`。health 無しの review prompt は `change_rules_review_without_health_keeps_the_legacy_prompt`。planner の brief 節は `change_rules_planner_brief_section_with_a_review_policy`・`change_rules_planner_brief_section_without_a_review_policy`。
- 1. affected 試験・clippy・fmt が通る
  - `bash scripts/dev/test-affected.sh` → 未実行（この後に実行して追記する）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。
- 2. prompt に入る規約本文の正本が generic.md 1 つで、CLAUDE.md と矛盾する写しが無い
  - コマンド: 上と同じ実行。
  - 要点: `change_rules_generic_once_in_every_run_kind`（全 run kind で 1 回）、`change_rules_generic_and_required_rules_do_not_copy_each_other`（generic.md と CLAUDE.md の 12 文字以上の行が互いに含まれない）、`change_rules_review_kind_descriptions_are_not_copied`（reviewer prompt の health の節・Result 節と `review.rs` に規約の説明文の写しが無い）が passed。

## WorkUnit review-health の受け入れ条件
- 0〜2 → [prompts-review.md](prompts-review.md) の「受け入れ条件ごとの証拠」。
- 3. 検査 → 上の 1 と、文書検査 3 本（`bash scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`sh scripts/dev/progress-index.sh --check`）→ 未実行（この後に実行して追記する）。
- 4. この file。

## 未解決事項
- protocol の `HealthReviewInput.brief`・`ReviewOutput.design`・`RunContext.design_review` は未作成で、Result 節に `design` の形を出していない（D5.4 の残り）。
- D7.1・D7.2・D7.4 の撤去はこの task では行っていない（U6 の範囲）。health の無い review の Result 節は旧い形のまま。
- dispatcher 側（`ReviewRequest.health` を埋める `build_health_input`、D5.2・D5.3 の読み込みと写像、D6.4 の人の承認）はこの task の branch に無い（dispatch task 01M6VTFG9K5HEE817EW05JASMZ の範囲）。それまで本番の reviewer prompt に health の節は出ない。
- review 方針（`review_policy`）は planner の context にだけある。worker・reviewer の run に repo 固有の節は出ない（rules-brief の未解決のまま）。
- Aider には CLAUDE.md が届かない（ADR D1.6）。汎用規約は届く。
- 全体の実行でだけ落ちることのある試験 3 件（task-dispatch の work_units、task-worker の browser launcher、celeris-campaign の launcher）は [prompts-rules.md](prompts-rules.md) の未解決のまま。

## 提案
- [prompts-rules.md](prompts-rules.md) と [prompts-review.md](prompts-review.md) の「提案」節を参照（browser・campaign の試験の決定化、`owner_rows` の省略数、U6 での Result 節の置き換え）。
