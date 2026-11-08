---
title: 健全性v2 G2 dispatch design-gate — 計画の採用前の risk 判定と高 risk だけの設計 review の起動
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch design-gate — 計画の採用前の risk 判定と高 risk だけの設計 review の起動

完了日: 2026-11-08

WorkUnit `design-gate`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D3.3・D3.5（1〜2）・D3.6。

## 変更
件名「G2 design-gate: 計画の採用前の risk 判定と高 risk だけの設計 review の起動」「G2 design-gate: 試験の仮の path を docs/ で始めない（doc.links が link と読む）」の commit と、この進捗の commit。
- `crates/task-dispatch/src/health_review/design.rs`（純粋な部分）: `write_paths_by_repo`（/3 の unit は `context.repo` ごと、repo の無い unit と /1・/2 の WorkUnit は先頭の repo）、`brief_required_classes`（`Present` の repo の高 risk の class の和）、`merge_risk`（repo ごとに `assess_risk` を呼び、最も高い level を採る。順位は none < low < unknown < high。repo が 2 つ以上なら理由に `<repo>:` を付ける）、`pending_design_review`・`design_review_started`（events だけで待ちを読む。I9）、`plan_summary`。
- `crates/task-dispatch/src/health_review/design_gate.rs`（Dispatcher の状態を使う配線）: `dispatcher.rs` から `#[path = "health_review/design_gate.rs"] mod design_gate;` で `dispatcher` の子 module として読む（`running`・完了の channel・`RunKey`/`RunEntry` が `dispatcher` の private なため。file は size の制約で `health_review/` に置く）。
  - `design_brief_gate`: 検証（退役した key の検査を含む）を通った planner の計画について repo ごとの方針を base 側で読み、高 risk の class の path を書くのに brief が無ければ `require_design_brief` の `DesignBriefMissing` を検証失敗の理由にする（既存の `give_up_or_retry_planner` の経路）。
  - `design_gate`（`tree_plan_gate`・子 task の検証の後、`adopt_plan_with_children`・`replan` の前）: `none`・`low` は今までどおり採用（level と理由は `tracing::info`）。`high`・`unknown` は `WorkerFinished`・quota・`WorkerProgress`・`DesignReviewRequested { plan_sha256, plan, risk, policy_sha256 }` を追記し、計画を採用せずに `Trigger::Continue { Planned }` で task を `ready` に戻す。
  - `design_review_hold`（`dispatch_one` の `decision_self_hold` と同じ条件式）: 待っている設計 review がある task には planner も WU も起こさない。その後に reviewer run が無ければ `pick_reviewer` で選び、`start_design_review` で起こす（選べなければ次の tick）。
  - `start_design_review`: `WorkerStarted { role: reviewer }`・runs 索引・route の選択の記録・quota の開始を final review と同じ形で書き、`run_reviewer_inner` と同じ形（合成の `Review` task、対象 task の dir、成果物 dir、run 専用の `TMPDIR`、前の `review.json` を消す）で adapter を起こす。文脈は `ReviewRequest.design`（`DesignReviewRequest { brief, plan_summary, risk, review_policy }`）だけで、summary・evidence・criteria は空。run は `running` に task の run として入る（並列度・cancel・同じ task の二重 dispatch の抑止）。新しい run 種別は作っていない。
  - `on_design_review_finished`: 新しい `Completion::DesignReview` で届く。`WorkerFinished`（role reviewer）・quota・runs 索引だけを書き、task の状態は変えない。`on_review_finished`・`review_verdict` には流さない。
  - `run_end_of`: run の終わり方（`RunEnd`・outcome の文・usage）の写像。`on_planner_finished` の冒頭の同じ写像（約 60 行）をここに移し、planner と設計 review の run で共有した（文は `planner` を入れると以前と同じ）。
- `crates/task-dispatch/src/dispatcher/planner_flow.rs`: `design_brief_gate` の呼び出し 1 行、`Ok((validated, gate))`、`design_gate` の呼び出し 3 行、冒頭の写像を `run_end_of` に置き換え。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs`: `decision_self_hold` の条件に `|| self.design_review_hold(&task)?` を足した（1 行）。
- `crates/task-dispatch/src/dispatcher.rs`: `mod design_gate`、`Completion::DesignReview` の variant と `drain_completions` の 1 行。
- `crates/task-dispatch/src/health_review/policy.rs`: base に `repo_health.py` が無い repo を `Unavailable` でなく `Absent` にした（下の「判断」）。
- 試験（prefix `health_review_design_gate_`、8 件）:
  - `crates/task-dispatch/src/dispatcher/tests/health_review_design_gate.rs`（5 件。一時 git repo に stub の検査器を commit）: 低 risk の計画は採用され reviewer run が 0 本、高 risk の path（`^crates/task-store/`）と `new_state` の brief の計画は `DesignReviewRequested`（level high・理由 `path_class:store+new_state`・`policy_sha256`）が書かれ未採用で設計 review の run が 1 本だけ起き、その run の後も `ReviewVerdict`・`ExecutionPlanned` が無く task は `ready`、WU は起きない。方針を読めない（非 0 exit）なら低 risk の path でも `unknown` で設計 review（fail closed）。brief の無い高 risk の計画は `design_brief is required` で planner の再試行に回り 2 回目の低 risk の計画が採用。`source = absent` は高 risk に見える計画でも従来どおり採用。
  - `crates/task-dispatch/src/health_review/design/tests.rs`（3 件）: repo ごとの write path、複数 repo の最も高い level と `policy_sha256`・brief を求める class、events だけの待ち。

## size の制約
`check-health.sh` は verdict=pass・new=0・worsened=0（existing 718）。size の行は空行・注釈・use・mod 宣言を除くので、配線で足した本体の行（約 7 行）は、`on_planner_finished` の冒頭の run の終わり方の写像を `design_gate.rs` の `run_end_of` に移して相殺した（`on_planner_finished` と module `dispatcher` の行は減った）。注釈の行を短くまとめた箇所（`dispatcher.rs`・`planner_flow.rs`・`dispatch_run.rs`）は行数の相殺には効いていない。

## 判断（ADR・前の unit との違い）
- base に検査器（`scripts/dev/repo-health/repo_health.py`）が無い repo を `Absent`（risk `none`）にした。policy-load は `Unavailable` にしていたが、そのままでは検査器を持たない repo（Celeris 以外の案件の repo、試験の repo）のすべての計画が fail closed で設計 review に回り、既存の試験 5 件（`work_units`・`stage_reopen`・`stalled_replan` の replan の試験）が落ちた。ADR の「`[review]` 節が無い repo は `none`」に当たる。base だけを見るので、候補が検査器を消しても足しても判定は変わらない（I3）。target を解決できない・非 0 exit・壊れた出力・時間切れ・remote は今までどおり `Unavailable`（fail closed）。policy の試験は `health_review_policy_script_missing_on_base_is_absent` に改めた。
- 設計 review を待つ間の task は `ready` に戻し、`dispatch_one` の hold で run を止める。新しい状態・列は足していない（I9）。待ちの判定は `DesignReviewRequested` と同じ `plan_sha256` の `DesignReviewed` の有無、run を起こしたかは後の `WorkerStarted { role: reviewer }` で、デーモンの再起動の後も同じになる。
- `DesignReviewRequest.review_policy` は event に本文が無い（`policy_sha256` だけ）ので、run を起こすときに方針を読み直し、`definition_sha256` が event と同じものだけを渡す（変わっていれば `None`）。
- level の順位は none < low < unknown < high にした（どちらも設計 review を起こす。high は宣言と path から決まった理由なので上に置く）。

## 証拠（提出時の HEAD で実行）
この WU では main の取り込み・rebase をしていない。下の検査は最後の code の commit（件名「G2 design-gate: 試験の仮の path を docs/ で始めない …」）の後に実行した（後の commit は進捗の file だけ）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。check-health は verdict=pass・new=0・worsened=0（existing 718）、repo_health_gate の試験は全部通過、nextest は 3519 passed・11 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p task-dispatch -E 'test(health_review_) | test(planner) | test(plan_invalid) | test(replan) | test(v3_)'` → exit 0、87 passed（`health_review_design_gate_` 8 件、`health_review_policy_` 12 件を含む）
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は `crates/task-dispatch/` の 11 file とこの進捗だけ（範囲外なし）

## 未解決（後続の unit へ）
- 設計 review の run の結果（`review.json` の `design`）は読んでいない。`DesignReviewed` の追記、合格での計画の採用、不合格での replan、2 回続いた不合格の `PlanGate` は design-verdict。それまで設計 review を待つ task は `ready` のまま止まる。
- 設計 review の run が infra の失敗（adapter の error・requeue）で終わっても、この unit ではやり直さない（`WorkerStarted` が既にあるので hold が待ち続ける）。やり直しの規則は design-verdict で決める。
- reviewer の prompt はまだ `ReviewRequest.design` を描かない（prompts task の review-health）。
- 設計 review の run には task 木の token 予算の watch（final review の `token_budget`）を付けていない。
- `design_review_hold` は Ready の task の dispatch のたびに events を読む。dispatch の頻度が問題になれば、型付きの問い合わせに替える。
- `pick_reviewer` の `subject_run_id` には直前の planner run の id を渡している。

## 提案
- 設計 review の待ちを人に見せる表示（task の待ちの理由 `TaskWait`）を design-verdict か web の unit で足す。今は `WorkerProgress` の 1 行だけ。
