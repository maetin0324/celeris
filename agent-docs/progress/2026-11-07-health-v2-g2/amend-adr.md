---
title: 健全性v2 G2 amend-adr — 二度目の final review の差し戻しへの方針を ADR の付記に書く
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 amend-adr — 二度目の final review の差し戻しへの方針を ADR の付記に書く

完了日: 2026-11-08

WorkUnit `amend-adr`。task 01M6TYVJSTYGRPHERB93MM7J59 の二度目の final review（reviewer run 01M6XA5X9W8V2DCAXMQ0631PR9。受け入れ条件 0・2・3 が不合格）に対する修正の方針を、[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の末尾の付記「付記（2026-11-08、二度目の final review の差し戻しへの方針）」に書いた。コードは変えていない。基準は `$CELERIS_WU_BASE`。

## 変更
- ADR の状態行を「実装済み」から「一部実装（二度目の差し戻しの修正中）」に変えた。
- ADR の末尾に付記を足した。決めたことは次の (a)〜(e) と流れ全体の試験の作り方。
- この進捗 file を足した。
- commit の件名: 「G2 amend-adr: ADR に二度目の差し戻しへの方針の付記を足し、状態を一部実装にする」、「G2 amend-adr: 付記の C1 を木の集計（Reviewer の run は reviewer_runs に数える）に合わせる」。
- C1 の訂正: `task_core::tree` の集計は Reviewer の run を `runs`（`max_tree_runs` の対象）でなく `reviewer_runs` に数え、費用と token には数える（`crates/task-core/src/tree/limits.rs`）。付記にはそのとおり書いた。

## 決めたこと（要点。本文は ADR の付記）
- (a) `change_rules/review.rs` に `design_section` を足し、`ReviewRequest.design` の brief の全欄・`plan_summary`・risk の level と理由・方針（owner_map・high_risk・invariants、読めなければその理由）を描く。Result 節は `result_section` の 1 節にし、設計 review では `{"verdicts":[],"design":{…}}`、final review では `repair` を含む `verdicts` と（あれば）`health` の形を出す。例の JSON は placeholder を使わず、`ReviewOutput` で読めることを試験で固定する。担当 design-prompt。
- (b) `RunContext.workspace_note` を `RunContext.workspace: Option<RunWorkspaceContext { note, review_policy }>` に置き換え、`ExecutionPlannerContext.review_policy` をそこへ移す（直下の欄数は 40 のまま）。dispatcher は tick の中で target の解決だけをし、`load_review_policy` は `run_worker` の非同期部分（`spawn_blocking`）で run の起動時に 1 回読む。LLM は呼ばず、cache は置かない。worker の run（atomic・WU・repair）と planner の run に入れ、reviewer の run と CoS chat には入れない。読めないときは節を出さない（関門は design_gate と policy_review_gate が fail closed で持つ）。担当 worker-profile。
- (c) `tree_run_limit_hold` から `tree_limit_hold_for(task, next)` を分け、`design_review_hold` が設計 review の run の直前に `NextRun::Worker` で呼ぶ。上限超えは run を起こさず既存の `kind: limit` の決定。watch は `token_budget::run_watch` の 1 つの関数にまとめ、review_spawn.rs・dispatch_run.rs・design_gate.rs が使う。担当 design-budget。
- (d) remote workspace は `Unavailable` → unknown → 設計 review のまま（fail closed）。(a) の節で本物の reviewer が合格を書けるので、人の承認に回らない。
- (e) D7.2 は撤去する。D7.1 は決定を改めて節を残し、`CLAUDE.md` の書く側の規則を写した 1 文だけを消す（理由: review 前の rebase はどの repo にも起き、判定の規則はこの節にしか無い）。D7.4 は「Records follow ADR-0128」「Whole-workspace tests」の 2 箇条と `PRODUCTION_HOST_PLANNER_GUIDANCE` を消し、「CPU-burning load scripts」の箇条は残す（共用 host の人の規則で全 repo に当たる。責任者は人、撤去条件は host の規則を 1 か所に移す変更）。担当 retire-legacy。
- 流れ全体の試験は、偽の reviewer が本物の `build_review_prompt` の prompt を組み、その Result 節の例の JSON を書く形にする（高 risk・remote workspace・通常の変更）。担当 flow-tests。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 条件 0（付記があり (a)〜(e) を決めている）: `grep -n '^## 付記（2026-11-08、二度目の final review の差し戻しへの方針）' agent-docs/adr/2026-11-07-health-review-integration.md` と `grep -c '^### ([a-e])' …` の結果を下の「検査の結果」に書く。
- 条件 1（worker の run への profile の置き場所と読み方が、`RunContext` の直下の欄を増やさない形で決まっている）: 付記の (b) B1・B2。
- 条件 2（状態行が実装済みでない）: `grep -n '^- 状態:' …`。
- 条件 3（`crates/` に変更が無い）: `git diff --quiet "$CELERIS_WU_BASE" -- crates/`。

## 検査の結果（提出時の HEAD で実行）
- `grep -n '^## 付記（2026-11-08、二度目の final review の差し戻しへの方針）' agent-docs/adr/2026-11-07-health-review-integration.md` → 292 行目に 1 件。
- `grep -c '^### ([a-e])' agent-docs/adr/2026-11-07-health-review-integration.md` → 5（(a)〜(e) の小節）。
- `grep -n '^- 状態:' …` → 7 行目「一部実装（二度目の差し戻しの修正中。…」。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0（`crates/` の未追跡 file も 0 件）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は ADR と この進捗 file の 2 つだけ。
- `sh scripts/dev/check-doc-links.sh` → exit 0（ok）。`sh scripts/dev/check-adr-numbers.sh` → exit 0（410 files）。`sh scripts/dev/progress-index.sh --check` → exit 0。
- コードを変えていないので `test-affected.sh`・clippy・fmt は走らせていない（`crates/` の差分 0）。

## 未解決事項
- 付記の C4 の代償（予算の尽きた task で LLM を呼ばない run の行が最大 `max_reviewer_retries + 1` 件増える）は受け入れた。減らすなら設計 review の起動前に watch の超過を見て止める分岐が要る。
- 進捗の「変更」節の件名の訂正（差し戻しの理由 5.）は reclose が行う。
- RunContext の欄数 40 は手元の数え方（`pub` の欄の行）での値。`size.responsibility-growth` の数え方での確認は worker-profile が check-health で行う。

## 提案
- 共用 host の規則（CPU を焼く負荷の禁止など）を、repo の `CLAUDE.md` でなく host の 1 か所（例: `preamble::fixed_notes`）に置くかを人が決める。決まれば `PLANNER_CHECK_GUIDANCE` の CPU の箇条と `CLAUDE.md` の写しを同じ変更で整理する。
- `PLANNER_CHECK_GUIDANCE` に残る agent-platform 固有の箇条（`check-doc-links.sh`・`corepack pnpm`・`cargo fmt`・`web/`・`docs/` の diff の check）を、repo の profile（`[review]` 節など）か repo の `CLAUDE.md` に移す。今はほかの repo の planner にも出る。
