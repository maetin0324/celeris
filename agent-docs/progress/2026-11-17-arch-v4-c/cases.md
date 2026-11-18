---
title: アーキ v4 C cases — 6 ケースの実証（認可 1 行 High・普通の変更 Low・移動で回避不可・依存の悪化を止める・policy の自己承認不可・fail-closed）
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
completed: 2026-11-18
updated: 2026-11-18
---
# cases（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の D5（と付記 (i)〜(v)）。
基準は `$CELERIS_WU_BASE`（integrate-impl の後の木）。commit は件名で挙げる: `arch-v4 C cases: 6 ケースの試験 boundary_case_1_〜6_ と、評価済みの health の項目を落とす evaluate_all の不具合の修正`。

## 結論
- `boundary_case_1_`〜`6_` の 6 件を `crates/task-dispatch/src/dispatcher/tests/boundary_cases.rs`（新しい file。dispatcher の events と
  dispatcher の試験の道具を使うので `dispatcher/tests/` に置いた）に足し、6 件とも通る。
- ケース 1・2・3・5 は実 profile を当てた: この worktree の `scripts/dev/repo-health/`（`profiles/celeris.toml` を含む）と `docs/architecture-map.md` を
  一時 git repo の main に commit し、本物の検査器の `--print-review-policy` が base の commit から読んだ方針（13 class）で判定する。
  ケース 4 は B の汎用 Rust git fixture（a→b→c→a に a→c を足す）に本物の検査器を当てた。
- 失敗注入 3 回（下の表）で、各ケースの試験が守る規則を戻すと該当の試験が落ちることを確かめ、戻した（注入した版は commit していない）。
- 不具合 1 件を owner で直した: `health_review::review_item::evaluate_all` が評価済みの health の項目（`RepoHealth::Evaluated`）を
  `take()` で取り出して捨てていた（下の「見つけた不具合」）。

## 6 ケースの表
コマンドはどのケースも `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)'`（提出時の HEAD で 6 passed）。

| ケース | 試験名 | 入力と通る経路 | 結果（提出時の HEAD） | 失敗注入（戻した版）と落ちた試験 |
|---|---|---|---|---|
| 1 認可条件の 1 行 → High | `boundary_case_1_authz_one_line_is_high` | 実 profile。planner の計画は `crates/task-api/src/middleware.rs` の `require_admin` の条件 1 行（brief は `new_*` 無し）。dispatcher の設計の関門 → 設計 review の run（台本で合格）→ 採用。final review の材料 `build_final_review_material`（policy_only）を同じ task で 2 回: middleware.rs だけの差分と、middleware.rs と principal.rs の差分 | `DesignReviewRequested` が 1 件、risk `High`、理由 `path_class:authz:crates/task-api/src/middleware.rs`・`invariant:authz:api-principal-from-token`・`invariant:authz:cos-audit-same-tx`。設計 review の prompt（`design_section`）に path・不変条件の名前と根拠の file。owner 地図の行は base の地図から `owner_rows` で引いたものと一致（今の地図に middleware.rs の行は無い。下の「未解決」）。final review: 覆った組だけの差分は `diff_design` 無し、principal.rs を足した差分は `diff_design` の理由が `path_class:authz:…principal.rs` と authz の不変条件だけ。`design` の無い `review.json` は `DIFF_DESIGN_VERDICT_MISSING` で不合格、`design.pass = true` で合格 | 注入 B（`assess_risk` を積の規則に戻す: `reasons.is_empty() \|\| declared.is_empty()` なら Low）→ `one design review is requested`（left 0）で落ちた |
| 2 同等の普通の変更 → Low | `boundary_case_2_equivalent_plain_change_is_low` | 実 profile。計画は `web/src/App.tsx` と `crates/task-api/src/handlers/org.rs`（class 外の handler）。final review の差分は同じ 2 file の 1 行 | `DesignReviewRequested` 0 件、設計 review の run 0 本、計画は採用。普通の path 5 件（画面・class 外の handler・`docs/SPEC.md`・`dispatcher/work_units.rs`・`task-worker/src/claude_code.rs`）は実 profile で 5 件中 0 件が Low 以外。final review の `diff_design` 無し | 注入 C（`assess_risk` が常に High）→ `[RiskAssessment { level: High, reasons: [] }]`（設計 review の要求が出る）で落ちた |
| 3 移動で回避不可 | `boundary_case_3_move_cannot_escape` | 実 profile。計画は class 外の `crates/task-api/src/auth/principal.rs` だけを宣言（Low で設計 review なしに採用）。差分: `git mv principal.rs → auth/principal.rs`、`git rm middleware.rs` と別名 `auth/guard.rs` の追加、`#[path]` の先 `health_review/design_gate.rs` を `dispatcher/gate_wiring.rs` へ `git mv` して `dispatcher.rs` の `#[path]` を付け替え | 採用前は要求なし。final review の `diff_design` は `High` で理由に `path_class:authz:…principal.rs`（rename の元）・`path_class:authz:…middleware.rs`（消した path）・`path_class:review-gate:…health_review/design_gate.rs`（`#[path]` の元）。`design` の無い review は不合格 | 注入 A（材料の `diff_stat` の `--no-renames` を `--find-renames` に）→ `the moved class files need a design verdict`（`diff_design` が None）で落ちた |
| 4 依存の悪化を止める | `boundary_case_4_worsening_dependency_blocks` | B の汎用 Rust git fixture（`[review]` の無い `default.toml`、a→b→c→a の循環に a→c を足す head）。本物の検査器を base に commit し、`health_gate`（health の関門の入口）と final review の材料（reviewer run あり）を当てる | 関門の判定は `Repair`（不合格）で `deps.cycle` の repair の理由に `./src/a -> ./src/c`。reviewer の prompt の health の節（`health_section`）に `` `deps.cycle` `` と `` symbol=`./src/a -> ./src/c` ``。report の欄は変えていない | 注入 A（`summarize_report` が `symbol` を写さない）→ prompt に symbol が無く落ちた |
| 5 policy の自己承認不可 | `boundary_case_5_policy_self_approval_impossible` | 実 profile。候補が自分の `celeris.toml` の authz の middleware の path を外し（`src/middleware` → `src/middleware_never`）、同じ候補で middleware.rs を変える。policy だけの候補も作る | 候補自身の方針では middleware.rs は Low、base の方針では `path_class:authz:…middleware.rs`。final review の `diff_design` は base の方針（`definition_sha256` が base と一致）で High。reviewer が `approved = true` を書いても、混ぜた候補は不合格、policy だけの候補は `policy_review_gate` が `Approval` の子（Ready）を 1 件作り、`resolve_human_approvals` は `None`（人を待つ） | 注入 A（policy_only の材料の検査器を `reviewed_sha` から選ぶ）→ `the base policy judges the candidate`（`diff_design` が None）で落ちた |
| 6 fail-closed | `boundary_case_6_fail_closed` | (a) base の検査器が exit 3 で方針を読めない、(b) 正規表現 `(` の方針、(c) `reviewed_sha` が無い commit（差分を取れない）、(d) `diff_design` を渡した review で `design` 無し・`pass = false`、(e) reviewer run の無い review（policy_only）で覆われない組（`HealthInputJob::attach`） | (a) 採用前 `Unknown`（`review_policy_unavailable:`）で設計 review、低 risk の path でも採用しない。(b) `Unknown`。(c) final review の `diff_design` が `Unknown`。(d) どちらも不合格。(e) health の合格の項目が `UNCOVERED_WITHOUT_REVIEWER` と `path_class:authz:…middleware.rs` 付きの不合格 | 注入 B（`from_final_review` が `design` の欠落を合格にする）→ `unwrap_err()` on `Ok` で落ちた。注入 C（`evaluate_all` を `take()` に戻す）→ (e) で `evaluated` の panic で落ちた |

### 失敗注入の手順と全体の結果
3 回とも `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)' --no-fail-fast` を走らせ、`git checkout --`（`review_item.rs` は退避した版の写し）で戻した。
戻した後に同じコマンドで 6 passed を確かめた。

| 注入 | 戻した規則（file） | 落ちた試験 | 通った試験 |
|---|---|---|---|
| A | `health_review/input.rs`: `diff_stat` の `--no-renames` → `--find-renames`、`summarize_report` の `symbol` → `None`、policy_only の検査器を `target_sha` → `reviewed_sha` | 3・4・5（それぞれ上の表の理由） | 1・2・6 |
| B | `task-core/src/design_review.rs` の `assess_risk` を積の規則に戻す（宣言が無ければ Low）、`health_review/verdict.rs` の `from_final_review` が `design` の欠落を合格にする | 1（要求 0 件）・6（(d)）。3・5 も差分の risk が Low になって落ちた | 2・4 |
| C | `assess_risk` が常に High（過剰に止める）、`review_item::evaluate_all` を `take()` に戻す | 2（要求が出る）・6（(e)）。3・5 も落ちた（3 は class 外の計画にも要求が出る、5 は候補自身の方針でも High） | 1・4 |

## 見つけた不具合（owner で直した）
- `crates/task-dispatch/src/health_review/review_item.rs` の `evaluate_all`: `let Some(RepoHealth::At(dir)) = check.health.take() else { continue; }` は、
  `At` 以外（評価済みの `Evaluated`）の項目も `take()` で取り出して捨てる。`HealthInputJob::attach` に評価済みの項目を渡すと、health の項目が無言で消え、
  `fail_uncovered`（付記 (v)）が不合格にする対象も無くなる（fail-open）。今の production の呼び出し元は `At` だけを渡すので本番で起きた記録は無いが、
  同じ checks に `evaluate_all` を 2 回当てると 2 回目で項目が消える。`take_if(|h| matches!(h, RepoHealth::At(_)))` にして、`At` だけを取り出す（1 行）。
  検出する試験は `boundary_case_6_fail_closed` の (e)（注入 C で確認）。既存の `review_risk_policy_only_fails_with_class_and_path` は `fail_uncovered` を
  直接呼ぶので、`attach` の経路の消失を検出できない。

## 実装前 brief（7 項目）
1. 根本原因: D5 の 6 ケースが実 profile と実際の判定経路で 1 つの流れとして確かめられていない。`evaluate_all` が評価済みの項目を捨てる。
2. 責務 owner: 判定は `task_core::design_review::assess_risk`、final review は `health_review::input`・`verdict`、health の項目は `health_review::review_item`。
3. 既存の再利用先: `health_review_design_verdict` の `DesignScriptAdapter`・`Script`（可視性を `pub(super)` にしただけ）、`health_review_scenario::copy_checker`、
   `health_review_design_gate::checker_repo`、dispatcher の試験の `compound_task`・`worktree_dispatcher`・`run_until_idle`・`new_task`・`dispatcher`、`owner_rows`。
4. 選択肢: (a) ケースごとに既存の試験 module の子にする → 道具が 3 か所に分かれ、`#[path]` が増える。(b) `dispatcher/tests/` の 1 file に 6 件（採用）。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ。`evaluate_all` は分岐を足さず取り出しの条件を直した）。
6. 撤去対象: 無し（既存の試験は弱めていない・消していない）。
7. 不変条件: 上の表。安全不変条件の code（store・events・lease・credential・CoS 監査・selfdeploy）は変えていない。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)'`: 6 passed。`grep` で `fn boundary_case_[1-6]_` の番号は 6 種。
- `cargo nextest run -p task-dispatch -E 'test(/boundary_case_|review_item|health_review_input|reuse/)'`: 63 passed（`evaluate_all` の既存の試験を含む）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（約 713 秒）。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3,933 passed・11 skipped、doc test は ok、script bundle の SKIP 0 件。web の型検査・unit 試験は対象外（web/・docs/api/・task-api の通常依存の変更が無い）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を範囲の正規表現に当てる）: 範囲外 0 件。変更 path は `cases.md`・`dispatcher/tests/boundary_cases.rs`・`dispatcher/tests/health_review_design_verdict.rs`・`dispatcher/tests/mod.rs`・`health_review/review_item.rs` の 5 件。
- 全体の試験と release gate は走らせていない（release gate だけで走る。`agent-docs/adr/2026-10-20-full-tests-at-release-only.md`）。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）
- production Rust: `review_item.rs` +2 −1（コメント 1 行と条件の 1 行）。
- 試験: `dispatcher/tests/boundary_cases.rs` +789（新しい file）、`dispatcher/tests/mod.rs` +3、`dispatcher/tests/health_review_design_verdict.rs` +4 −4（可視性だけ）。
- 新しい reviewer 基盤・framework・event kind・config 欄は足していない。`scripts/dev/repo-health/` は変えていない。

## 未解決・提案
- 実 profile の owner 地図（`docs/architecture-map.md`）に、新しい class の file の多く（`task-api/src/middleware.rs`・`principal.rs`・`handlers/authority.rs`・
  `task-core/src/authority.rs`・`cos/operations.rs`・`task-ops/src/lifecycle.rs`・`task-core/src/transition.rs`・`task-worker/src/browser_policy.rs`・
  `dispatcher/worker_finish.rs`・`celeris/src/instance.rs`）の path も親 dir も無い（`knowledge.rs`・`replay.rs`・`design_review.rs` は有る）。
  そのため設計 review の prompt の owner 地図の行は空になる。地図に行を足すのは `docs/` の変更で、この WU の範囲の外（提案: H か docs の task が足す）。
- ケース 2 の「過剰に止めない」は普通の path 5 件と計画・差分の各 1 回で示した。区間の commit の replay の件数は WorkUnit `metrics` が出す。
- `cargo clippy -p task-dispatch --tests -- -D warnings` は既存の `dispatcher/tests/cex_lease_restart.rs:1168` の `type_complexity` で落ちる（この WU の変更ではない。
  repo の必須の検査は `--workspace` の lib だけで、exit 0）。

## 本番への移行
- config・DB の変更は無い。`evaluate_all` の修正は昇格の後の daemon から効く（昇格は人だけ）。production の呼び出し元は `At` だけを渡すので、挙動の変化は無い見込み。
