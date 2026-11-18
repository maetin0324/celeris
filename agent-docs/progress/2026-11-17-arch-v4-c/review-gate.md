---
title: アーキ v4 C review-gate — final review の差分（rename の両側）に risk を当て、覆われない (class, path) に diff_design で design verdict を要求する
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
completed: 2026-11-18
updated: 2026-11-18
---
# review-gate（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の D3 と「## 付記（2026-11-18、計画 v3 の決定）」(ii)〜(v)。
基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。main は merge していない。
commit は件名で挙げる: `arch-v4 C review-gate: final review の差分に base 側の方針で risk を当て、覆われない (class, path) の組に diff_design で design を求める`、
`arch-v4 C review-gate: 進捗に提出時の HEAD の検査結果を書く`。

## ADR D3 との違い（受け入れ条件 6）
- D3 は「`git diff --name-status --find-renames <base>..<head>` の path を集める」と書いたが、実装は新しい git の経路を足さず、材料が既に取る
  `diff_stat`（`git diff --numstat -z --no-renames <target>..<reviewed>`）の `changed` を再利用した。`--no-renames` では rename は「元の path の削除と
  先の path の追加」になるので、rename の両側と消した path が `changed` に入る（copy は先の path だけ。元の file は変わらないので risk の対象でない）。
  判定は同じ `task_core::design_review::assess_risk`（brief は渡さない）で、2 つ目の判定・探索関数は無い。
- 覆う範囲は付記 (iii) の (class, path) の組。D3 の「class 単位」は使わない。
- brief を渡さないので `assess_risk` が足す `brief_absent_human_plan`（計画に brief が無いことの語）は差分の risk から外した（`diff_risk`）。
- 複数 repo の計画の設計 review の理由（`<repo>:path_class:…`。`merge_risk` が repo 名を前に付ける）は組にしない（覆わない側に倒す）。final review の
  repo は id で、計画の repo は名前なので、名前と id の対応を引く経路を足さなかった。単一 repo の計画は今までどおり覆う。

## 実装前 brief（7 項目）
1. 根本原因: final review が実際の差分を class に当てず、計画の宣言だけで設計 review の有無が決まる。宣言の漏れ・rename・削除で境界の file を変えても設計の判定が無い。
2. 責務 owner: final review の材料 `health_review/input.rs`（差分を取る `diff_stat` と材料の記録）、reviewer の request `review.rs`（`into_request`）、`review.json` の読み `health_review/verdict.rs`。
3. 既存の再利用先: `diff_stat` の `changed`、`assess_risk`、`DesignReviewRequest`・`DesignVerdictOut`・`ReviewRequest.diff_design`（review-wire）、`HealthReviewInputBuilt`・`ReviewVerdict` の event、`fail_all`、health の項目（`HealthItem`）、`SUBSTANTIVE_SCOPE`。
4. 選択肢: (a) final review で別の git diff（`--find-renames`）を取る → 経路の二重化で不採用。(b) 材料の `changed` を再利用（採用）。policy_only の不合格は (c) `ReviewExtras` に欄を足して `review_task` に不合格の verdict を足す → `spawn_review`・`review_task`（どちらも size.responsibility-growth の existing の単位）に行が増えるので不採用、(d) 評価済みの health の合格の項目を不合格にする（採用）。
5. 新しい状態・分岐・fallback・port: 状態と event kind は無し。分岐は「覆われない組があれば `diff_design` を付ける」と「reviewer run が無ければ health の項目を不合格にする」の 2 つ。fallback は無し（読めない・取れない・材料を作れないは `Unknown`）。port は wire の任意の欄 3 つ（`HealthRepoInput.risk`・`HealthReportFinding.symbol`・`rationale`）。dispatcher の内部の型 `FinalReviewMaterial`（`ReviewerRun.health` を `material` に置き換え、health と diff_design を 1 回の材料作りから運ぶ）。
6. 撤去対象: `ReviewerRun.health`（`material.health` に置き換え）、`build_health_input`・`build_health_input_with` の production の入口（`#[cfg(test)]` の試験の入口だけ残す。production は `build_final_review_material`）、review-wire の `diff_design: None` の注記。
7. 不変条件と試験: 下の表。安全不変条件の code（store・events・lease・credential・CoS 監査・selfdeploy）は変えていない。

## 変更
- `crates/task-dispatch/src/health_review/input.rs`:
  - `repo_input` が base 側の方針の状態を保ち、`diff_stat` の `changed` に `diff_risk`（`assess_risk(state, changed, None)` から `brief_absent_human_plan` を外す）を当てて
    `HealthRepoInput.risk` に入れる。記録は既存の `HealthReviewInputBuilt.input`。`find_recorded` は `risk` の無い記録（D3 より前の daemon の記録）を再利用しない。
  - `build_final_review_material`（production の入口）: health の材料に加え、repo ごとの risk（`RepoRisk`）を集め、採用した計画の `plan_sha256` で
    `covered_pairs`（最後の `DesignReviewed` が合格のときだけ、同じ sha の `DesignReviewRequested.risk` の `path_class:<class>:<path>`）を引き、
    `uncovered`（覆われない path と、残った class の `invariant:`）を `diff_design`（要約は repo ごとの差分の stat、risk は残りの理由、方針は base 側、brief は採用した計画）にする。
  - fail-closed: 方針を読めない・適用判定が `Unavailable`・差分を取れない repo（skip で policy が不明の repo）は `RepoRisk::unknown`（`review_policy_unavailable:<要約>`、level `Unknown`）。
    正規表現を読めない方針は `assess_risk` が `Unknown` を返す。材料作りの task が落ちた（`JoinError`）ときも `Unknown` の `diff_design`。方針が `Absent` の repo は risk を作らない。
  - policy_only（reviewer run の無い review）: `record_policy_only` が毎回 `candidate_risk`（base 側の方針と `diff_stat`）を決める（既知の policy の状態があっても risk は決める）。
    `HealthInputJob::attach` は reviewer run が無ければ `fail_uncovered` で評価済みの health の合格の項目を不合格にする（理由 `diff_design: …: <class・path の理由>`、
    repair は `substantive`）。もともと不合格の項目（`health_check_error` 等）は元の理由と扱いのまま。
  - `summarize_report`: `HealthReportFinding.symbol` に最初の evidence の `symbol`、`rationale` に finding の `rationale` を写す（鍵 `cycle-edge:` は組み直さない）。
- `crates/task-dispatch/src/review.rs`: `ReviewerRun.health` → `material: FinalReviewMaterial { health, diff_design }`。`into_request` が `diff_design` を
  `ReviewRequest.diff_design` に入れ、`design` は常に `None`。`run_reviewer_inner` は `Verdict::from_final_review` で読む（行数は同じ）。
- `crates/task-dispatch/src/health_review/verdict.rs`: `Verdict::from_final_review` — `from_review_json` の後、`diff_design` を渡した review で
  `design` が無い（`DIFF_DESIGN_VERDICT_MISSING`）・`pass = false` なら `Err`（呼び出し側の `fail_all`。局所修復なし）。壊れた `design` は `ReviewOutput` の読み込みで落ちる。
- `crates/task-dispatch/src/health_review/gate.rs`: `RepoHealthEvidence.symbol`（report v1 の既存の欄を読むだけ）。
- `crates/task-worker/src/protocol/health.rs`: `HealthRepoInput.risk?`、`HealthReportFinding.symbol?`・`rationale?`（任意の欄。省略した JSON は従来どおり読める）。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `pick_reviewer` の `health: None` → `material: Default::default()`（行数は同じ）。
- 生成 schema `docs/protocol/worker-protocol.schema.json`（`UPDATE_SCHEMA=1 cargo nextest run -p task-worker -E 'test(/schema/)'`）と `docs/protocol/worker-protocol.md` の §10.2 の形。
- `ReviewerPassReused` は変えていない（付記 (iv)）。event 種別・reviewer run の種類・`scripts/dev/repo-health/` は足していない・変えていない。

## 試験（守る不変条件と、既存の試験で検出できない理由は各試験の注釈）
| 試験 | 確かめること |
|---|---|
| `review_risk_rename_delete_and_path_target_are_assessed` | 一時 git repo で `git mv principal.rs`・`git rm middleware.rs`・`#[path]` で読む `health_review/design_gate.rs` の変更 → 材料と `HealthReviewInputBuilt.input.risk` に `path_class:authz:<元の path>`・`path_class:authz:<消した path>`・`path_class:review-gate:…design_gate.rs`・`invariant:authz:api-principal-from-token`。計画の無い task は全部を `diff_design` で求める |
| `review_risk_plain_change_needs_no_design` | class に合わない `web/src/App.tsx` の変更は `Low`、`diff_design` 無し |
| `review_risk_covered_pair_is_not_required` | 合格した設計 review の組（同じ path・`/` で終わる dir の接頭辞）は要求しない |
| `review_risk_same_class_other_path_is_required` | 同じ class の別 path は要求する。不合格・採用していない計画・計画の sha 無し・複数 repo の理由は何も覆わない |
| `review_risk_design_missing_or_failed_is_not_pass` | `diff_design` を渡した review で `design` 無し・`pass=false`・壊れた `design` は `Err`（fail_all）。`diff_design` 無しは従来どおり |
| `review_risk_unknown_requires_design` | 方針を読めない（reviewer 有り・無し）・正規表現を読めない・差分を取れない → `Unknown`。`Absent` は高 risk の path でも何もしない |
| `review_risk_policy_only_fails_with_class_and_path` | policy_only で覆われない組 → health の合格の項目が class・path 付きの不合格（substantive）。不合格の項目はそのまま |
| `review_risk_final_review_sets_diff_design_not_design`（`review/tests.rs`） | `into_request` は `diff_design` だけを付け、`design` は `None` |
| `review_risk_finding_symbol_from_real_checker` | 本物の `repo_health.py` を `python3 -I -B` で一時 git repo（a→b→c→a に a→c を足す head）に当て、`summarize_report` が `symbol = "./src/a -> ./src/c"`、`rationale` に「新しい組」 |

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
（下の節を提出時の HEAD の結果で埋める）

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）
- production Rust: +447 −77（純増 370）。内訳 `health_review/input.rs` +369 −65、`review.rs` +22 −11、`health_review/verdict.rs` +41、`protocol/health.rs` +11、
  `health_review/gate.rs` +3、`dispatcher/review_spawn.rs` +1 −1。うち doc comment が約 50 行。
- 試験: +567 −2（新しい file `health_review/input/tests/risk_tests.rs` 519 行）。生成 schema +25。文書 +5 −1。
- size.responsibility-growth の existing の単位（`Dispatcher::spawn_review`・`pick_reviewer`・`review::review_task`・dispatcher の module）は行を純増させていない
  （`pick_reviewer` は 1 行の置き換え、`run_reviewer_inner` は行数同じ、`Dispatcher` の method は足していない）。

## 未解決・提案
- 規模: ADR の「production の Rust の純増の目安は 400 行以内」（C 全体）に対し、risk-rule 29・review-wire 94・この WU 370 で計 493 行になり、目安を超えた。
  prompts の leaf は小さく保つ必要がある。closeout か H が「目安の超過を受け入れるか、`input.rs` の `RepoRisk`・`diff_design` を縮めるか」を判断する（提案）。
- 同期の無い repo（`NOT_SYNCED`）と remote workspace の repo は差分が無いので risk を当てない（今までどおり health の材料も無い）。fail-closed にするかは H で確かめる。
- 複数 repo の計画の設計 review は final review の組を覆わない（上の「違い」）。覆わない側なので安全だが、複数 repo の task は差分の設計の判定を毎回求められる。
- `cases` の leaf（D5 の 3・4・6）は、この WU の `build_final_review_material`・`Verdict::from_final_review`・`fail_uncovered`・`summarize_report` の `symbol` を使える。
  reviewer の prompt に `symbol`・`rationale` を出すのは `prompts` の leaf（`change_rules/review.rs` の health の節）。

## 本番への移行
- config・DB の変更は無い。D3 を含む release が昇格した後の daemon から効く（昇格は人だけ）。
- 昇格の後、`risk` の無い `HealthReviewInputBuilt`（昇格の前の記録）は再利用せず作り直す（repo health を 1 回余計に走らせる）。
- 昇格の前に出た reviewer の合格が `ReviewerPassReused` で昇格の後に再利用されると、その candidate には差分の設計の判定が求められない（付記 (iv) の限界）。
- 昇格の後、計画の無い task・設計 review を経ていない計画の task で高 risk の class の path を変えると、final review で `design` を求められる。
  reviewer run の無い task（command の check だけ）は覆われない組があれば health の項目で不合格になる（付記 (v)）。
