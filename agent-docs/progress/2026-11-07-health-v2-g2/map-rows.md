---
title: 健全性v2 G2 map-rows — architecture-map の『健全性の review』の行を今の実装に合わせ、汎用規約と review 方針の配布の行を足す
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 map-rows — architecture-map の『健全性の review』の行を今の実装に合わせ、汎用規約と review 方針の配布の行を足す

完了日: 2026-11-08

WorkUnit `map-rows`。task 01M6TYVJSTYGRPHERB93MM7J59 の三度目の final review（reviewer run 01M6Y1RWMFQHQCWV2Z69BGMT9M）は、受け入れ条件 3 を不合格にした。その差し戻しのうち (1)（地図の「健全性の review」の行が古い）と (2)（『共通 agent 指示と配布』に汎用規約の配布の行が無い）を直した。(3)（`crates/task-core/src/tree/limits.rs` の doc comment）は並行する WorkUnit `limits-doc` の担当で、ここでは触っていない。コード（`crates/`）、ADR、task の進捗 file（`agent-docs/progress/2026-11-07-health-v2-g2.md`）は変えていない。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
- commit の件名: 「G2 map-rows: architecture-map の『健全性の review』の行を今の実装に合わせ、汎用規約と review 方針の配布の行を足す」、「G2 map-rows: 進捗に地図の行の変更・docs の旧い名前の検索結果・検査の結果を書く」。
- `docs/architecture-map.md` の「健全性の review」の行（(A)）
  - 概要: 「planner への受け渡し」を「planner・worker の run への配布〈`RunContext.workspace.review_policy`〉」に改め、「設計 review の run への木の run・費用の上限と token 予算」を足した。
  - 入口: もう無い `Dispatcher::planner_review_policy` を消した。代わりに次の経路を書いた。`health_review/worker_profile.rs` は `dispatcher/run_context.rs` の子 module。tick の `run_extras` で `Dispatcher::run_workspace` が `Dispatcher::review_policy_target` で repo と target を決める。run の起動（`dispatcher/worker_task.rs` の `run_worker`）では、`RunWorkspace::into_context` が `load_review_policy` を 1 回呼ぶ。設計 review の予算の入口は 2 つ。`Dispatcher::design_review_hold` が `dispatcher/tree_units.rs` の `Dispatcher::tree_limit_hold_for` を呼ぶ。`start_design_review` が `dispatcher/token_budget.rs` の `run_watch` を呼び、その adapter は `review.rs` の `run_within_token_budget` の下で走る。名前と場所は実装（`health_review/design_gate.rs:187`・`:259`・`:284`・`:492`、`dispatcher/tree_units.rs:795`、`dispatcher/token_budget.rs:56`、`review.rs:1075`、`dispatcher/run_context.rs:392`、`dispatcher/worker_task.rs:463`）を読んで確かめた。
  - ADR の欄: 付記「二度目の final review の差し戻しへの方針」の (b)・(c) を足した。
  - 関連試験: `crates/task-dispatch/src/health_review/worker_profile/tests.rs` を足した（既存の `{policy,…,policy_review}/tests.rs` の brace に `worker_profile` を加えた）。`crates/task-dispatch/src/health_review/flow_tests.rs`（`crates/task-dispatch/src/dispatcher/tests/mod.rs` の `mod health_review_flow`）と `crates/task-dispatch/src/dispatcher/tests/health_review_design_budget.rs` も足した。
  - 不変条件: worker・planner の run の方針は run の起動で 1 回だけ読み、読めなければ入れないこと（付記 (b)、試験 `health_review_worker_profile_`）を足した。設計 review の run も木の上限と token 予算の下で走ること（付記 (c)、試験 `health_review_design_budget_`）も足した。既存の I3・I4・I5・I7・I8・I9 の文は変えていない。
- `docs/architecture-map.md` の『共通 agent 指示と配布』節（(B)）: 必須規則の行の直後に 1 行を足した（地図は 213 行から 214 行）。
  - 入口: 汎用規約の正本 `crates/task-worker/src/change_rules/generic.md`（`change_rules.rs` の `GENERIC_RULES`・`section`）と repo 固有の節 `repo_section`、組み立て口の `crates/task-worker/src/claude_code/prompt.rs` の `prompt_header`、方針の配布の `crates/task-dispatch/src/health_review/worker_profile.rs`（`Dispatcher::run_workspace`・`RunWorkspace::into_context`）。
  - 関連試験: `crates/task-worker/src/change_rules/{tests,worker_profile_tests}.rs`（`change_rules_generic_`・`change_rules_worker_profile_`）と `crates/task-dispatch/src/health_review/worker_profile/tests.rs`（`health_review_worker_profile_`）。
  - 不変条件: 汎用規約の正本は 1 file で、`CLAUDE.md` と互いに写さない。adapter と CoS chat では足さない。方針は base 側から読み、候補の profile を読まない（I3）。読めなければ節を出さない。根拠は ADR D1・付記 (b)。
- (C) docs/ の旧い名前: `planner_review_policy`・`tree_run_limit_hold`・`ExecutionPlannerContext.review_policy`・`workspace_note` を docs/ 全体で探した。今の仕組みとして書いていたのは `docs/architecture-map.md:66` だけだった。`docs/protocol/worker-protocol.md`（`context.workspace` の行）と `docs/protocol/worker-protocol.schema.json`（`review_policy` を持つ型は `RunWorkspaceContext` と `DesignReviewRequest` だけ）は、すでに今の形を書いていた。`agent-docs/adr/` と `agent-docs/progress/` には旧い名前が残る。どちらも当時の記録なので変えていない。

## 目的文との違い
- 『共通 agent 指示と配布』の行では、`worker_profile.rs` と `dispatcher/run_context.rs` を全角括弧に入れず、どちらも repo からの path で書いた。`check-architecture-map.py` は、backtick の path の直後の全角括弧の中の path を、前の path の dir からの相対 path として展開する。この規則のため、`worker_profile.rs`（`dispatcher/run_context.rs` …）と書くと `health_review/dispatcher/run_context.rs` を探して落ちる。「健全性の review」の行の入口では、括弧の前の path が `crates/task-dispatch/src/health_review.rs` なので、目的文のとおり相対 path で書けた。入れ子の全角括弧も同じ理由で括弧を閉じてしまうので、括弧の中の補足には〈〉を使った。
- token 予算の watch の入口は、目的文が「正確な名前は実装を読んで確かめる」とした所。実装では `design_review_hold` は木の上限だけを見る。watch を組むのは `start_design_review` で（`token_budget::run_watch`）、adapter を watch の下で走らせるのは `review::run_within_token_budget` だった。そのとおりに書いた。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 scripts/dev/check-architecture-map.py` → exit 0（「OK: 967 件のパスを確認した」。変更前は 956 件）
- `python3 scripts/dev/check-architecture-map.py --diffs --reference agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json` → 変更前も変更後も 142 件。種類別の件数も同じで、鍵の差は増減とも 0 件
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- `git diff --name-only "$CELERIS_WU_BASE"..HEAD` → `docs/architecture-map.md` と `agent-docs/progress/2026-11-07-health-v2-g2/map-rows.md` の 2 件だけ（`crates/` の変更なし）
- `bash scripts/dev/test-affected.sh --list` → exit 0。対象の crate は 0 件（docs だけの変更）
- `bash scripts/dev/test-affected.sh` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 地図は 214 行で、「この文書の作り方・保ち方」の 200 行の目安を超えている。変更頻度の低い行をまとめる作業は、この WorkUnit の範囲（1 行まで足す）の外なので行っていない。
- 三度目の差し戻しの (3)（`limits.rs` の doc comment）は WorkUnit `limits-doc` が直す。ADR の付記と task の進捗への記録は後続の `reclose-docs` が書く。

## 提案
- `check-architecture-map.py` は path の実在しか見ない。そのため、表の中の `Dispatcher::planner_review_policy` のような関数名が消えても検出できない。backtick の `<型>::<fn>` の形の名前について、その行の入口の file（brace と相対展開の後）に `fn <fn>` があるかを確かめる検査を足すとよい。足すかどうかは、誤検出（trait の method・macro が作る名前）の量を見て人が決める。
