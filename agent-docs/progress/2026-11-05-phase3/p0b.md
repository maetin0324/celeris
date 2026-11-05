---
title: P0-B 達成判定の真偽と Roadmap 接続（Rust＋狭い TS）
tasks: [01M6N7XY9H31GK4K0G0J1C6AQY]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-B 達成判定の真偽と Roadmap 接続（Rust＋狭い TS）

完了日: 2026-11-05。親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`。設計は ADR `agent-docs/adr/2026-11-05-phase3-achievement-invariants.md`（I1〜I4・I11、K1〜K12・K41）、この task の決定は付記 `agent-docs/adr/2026-11-05-phase3-p0b.md`。

## 結論
- phase `achieved` は、Rust が TS の `gap` を使わずに数え直し、全 Outcome に「I3 を満たす root」と「到達の記録」があるときだけ書く。欠ければ phase は `waiting`、判断の行に `pc/achieved-unverified:<未達の id>` を残す（I1）。
- `mark_milestone` は効果 `RecordOutcome` になった。効果の時に Rust が根拠を確かめ直し、approved の Roadmap があれば同じ tx で門を読んで `reached` を書く。Roadmap が無ければ適用済みの判断の行が正本（I2）。
- 根拠の root の条件を絞った: 最後の終端遷移が `review_pass` の done、最後の review に不合格が無く command の検査が全部合格、repo 付きは `IntegrationMerged`、repo 無しは成果物（I3・I4）。
- TS（control-ts）は変えていない。TS の `satisfies` は snapshot の `reviewer_passed`・`integrated` を読むので、Rust が絞った欄に自動で従う。TS だけの変更が無いので、verify の cargo 0・p50/p95 の条件はこの task では対象外。
- migration は足していない（0115 の予約は未使用。P1-G は 0115 を使ってよい）。

## 照合した資料（修正済みを再実装しない）
`agent-docs/progress/2026-11-04-long-horizon.md`・`a1.md`〜`a9.md`、`2026-11-05-a9-llm-pilot.md`、`2026-11-05-a9-fix-arrival.md`、ADR `2026-11-05-a9-llm-pilot-findings.md`、`docs/ops/project-controller.md`、P0-A の `p0a.md`。A9 D1 の「repo 付きは `IntegrationMerged` だけ」は保った（`root_row_repo_*` 4 件が通る。K12）。旧 milestones API の 410 も保った（`milestone_lifecycle_endpoints_are_gone` 等 2 件が通る）。

## 変更（commit の件名）
- `p0b: 達成を Rust が数え直し、mark_milestone を到達の記録に結ぶ（I1〜I4・I11）`
  - `crates/task-core/src/project_proposal.rs`: `validate` から `gap` による達成の判定を消し、`unverified_outcomes` を足した。
  - `crates/task-core/src/store/roadmap.rs`: `roadmap_last_status_tx`・`roadmap_effect_gate_open_tx`（効果の tx の中で読む）。
  - `crates/task-ops/src/project_snapshot.rs`: `root_row` の I3(b)(c)・I4。単体試験 6 件を足し、`root_row_without_repo_done_is_integrated` を I4 の陽性・陰性 2 件に置き換えた。
  - `crates/task-dispatch/src/dispatcher/project_decider_ts.rs`: `recorded_outcomes`・`verify_achievement`・`record_outcome`・`outcome_in_place`、`MarkMilestone` の効果。
  - `crates/task-dispatch/src/project_controller.rs`: `ProjectEffect::RecordOutcome`。
  - `crates/task-dispatch/src/dispatcher/project_wake.rs`: `apply` と `effect_in_place` に `RecordOutcome` の arm を 1 つずつ（P1-G の持ち物。統合のときは arm を残す）。
  - `crates/task-dispatch/src/dispatcher/tests/project_achievement.rs`（新規、`project_cos` の子 module）: 試験 12 件。
- `p0b: 既存の試験の fixture を I4 に合わせ、ADR の付記を足す`
  - repo の無い root を done にする fixture に `ArtifactProduced` を足した（`long_horizon_pilot.rs`、`crates/celeris/tests/project_controller_e2e.rs`、`crates/task-api/tests/project_roadmap.rs`）。
- `p0b: 進捗を書く`（この file と raw）

## 受入とテストの対応
| 受入 | ケース | 試験（新規は太字なし・全部 `project_achievement_` か `root_row_`） |
|---|---|---|
| 0 未取り込み | K1・K12 | `project_achievement_unintegrated_root_is_not_achieved`（`NoCommits` では未達、`IntegrationMerged` の後に記録 1 件で達成） |
| 0 root 無し（TS が gap を偽る） | K1b・K2 | `project_achievement_forged_gap_without_root_is_refused` |
| 0 reviewer だけ pass・検査 fail | K3 | `project_achievement_command_check_fail_is_not_reached`、`root_row_command_check_fail_is_not_passed` |
| 0 違う案件 | K4 | `project_achievement_other_project_root_is_refused`（効果を直接呼んでも `refused:roadmap_evidence_unknown`、記録 0） |
| 0 一部 Outcome のみ | K5 | `project_achievement_partial_outcomes_wait`（3 件中 2 件で `pc/achieved-unverified:o3`） |
| 0 全 Outcome で達成 | K6・K41 | `project_achievement_all_outcomes_reached_roadmap`（Roadmap の `reached` が Outcome ごとに 1 件、成果物の無い root では未達）、`project_achievement_all_outcomes_reached_request_block` |
| 1 版の差し替え | K7 | `project_achievement_roadmap_version_swap_reopens`（achieved の後に Outcome を足す版を承認 → `pc/achieved-unverified:o2`、o1 の記録は 1 件のまま） |
| 1 重複 event | K8 | `project_achievement_duplicate_events_record_once`（`IntegrationMerged`・`ReviewerPassed` を 3 回、TS が同じ mark を繰り返しても記録 1 件） |
| 1 reopen | K9 | `project_achievement_reopened_root_is_not_evidence`、`root_row_reopen_drops_previous_pass`、`root_row_done_without_review_pass_is_not_passed` |
| 1 並行 approval/CAS | K10 | `project_achievement_concurrent_approval_cas`（同じ版を土台の承認 2 件は 1 件だけ通り、もう 1 件は `Stale`。承認の後の古い判断の効果は門で `GateClosed`、記録は増えない） |
| 1 再起動 | K11 | `project_achievement_restart_records_once`（同じ判断を当て直しても記録 1 件、`outcome_in_place` が記録を見つける、daemon を作り直して評価を続けても 1 件） |
| 2 repo 付きは `IntegrationMerged` だけ | K12 | 既存 `root_row_repo_no_commits_is_not_integrated`・`root_row_repo_not_git_is_not_integrated`・`root_row_repo_integration_merged_is_integrated`・`root_row_repo_not_applicable_is_not_integrated` |
| 3 `test -f …/p0b.md` | — | 下の証拠 |
| (objective) repo の無い調査案件の証拠規則 | K41 | `root_row_without_repo_with_deliverable_is_integrated`・`root_row_without_repo_without_deliverable_is_not_integrated` |

試験名の一覧（`cargo nextest list … --message-format oneline`）は `p0b/tests-list.txt`（57 件）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 選んだ試験: `cargo nextest run -p task-ops -p task-dispatch -p task-api -p celeris --no-fail-fast -E 'test(/project_achievement_|root_row_|project_roadmap_|project_controller_core_|long_horizon_pilot_|milestone_lifecycle_endpoints_are_gone|milestones_are_gone/)'` → exit 0、57 passed（`p0b/selected-tests.txt`）
- `bash scripts/dev/test-affected.sh` → exit 0、7633 passed・21 skipped、script bundle の SKIP 0 件（`p0b/test-affected-tail.txt`）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → 下の「最後の同期の後の再実行」を参照
- `test -f agent-docs/progress/2026-11-05-phase3/p0b.md` → exit 0

## 失敗ケース（途中で落ちたもの）
- 初回の `cargo nextest run` は v8 の link で落ちた（`could not find native static library rusty_v8`）。target の `debug/build/v8-*` を消して作り直した。
- I4 を入れた直後、repo の無い root を `ArtifactProduced` なしで done にする既存の試験 7 件が落ちた（`root_row_without_repo_done_is_integrated`、`long_horizon_pilot_*` 4 件、`project_roadmap_root_links_and_reached_need_rust_evidence`、`project_controller_core_s2_s3_root1_to_root2_to_achieved`）。前者は I4 の陽性・陰性の試験に置き換え、残りは fixture に成果物を足した（試験の期待値は変えていない）。
- 新しい陰性の試験が修正前のコードで赤になることは、この run では実行して確かめていない。修正前の `validate` は `gap.open` が空なら achieved を通し、`MarkMilestone` は効果を持たなかったので、K1b・K6 は修正前に落ちるはず（未確認）。

## 未解決
- decide と commit の間に root が reopen されると、phase `achieved` が書かれうる。commit（`project_controller_commit`、P1-G の持ち物）の CAS は revision・human_epoch だけを見る。achieved の後は人の変更の wake でしか評価しないので自動では戻らない。P1-G との統合で、commit の tx に到達の数え直しを足すか、root の reopen を achieved の案件の wake にするかを決める。
- 実 LLM での確認はしていない（P0-F の再試験で、controller の achieved と oracle の一致を測る）。
- `docs/ops/project-controller.md` への反映（`pc/achieved-unverified`・`RecordOutcome`・I4 の成果物の規則）は P2-J に渡す。
- shadow・dry_run では `suppressed` の mark を到達の記録に数える。enabled に上げた直後は記録が無いので、mark をやり直してから achieved になる（期待どおりと判断したが、運用の文書には未記載）。

## 次の gate
- P1-G との統合: `project_wake.rs` の `RecordOutcome` の arm 2 つを残す。migration 0115 は P0-B が使っていない。
- P0-F（実 LLM の再試験）は P0-B〜P0-E の完了後、人の明示の許可で。
- P2-J: GET・UI に `pc/achieved-unverified` と未達の Outcome の id を出す（K43）。

## 提案
- root の reopen（`Transitioned` で done から非終端）を、phase `achieved` の案件の wake にする（上の未解決 1 を P1-G の wake 側で閉じる）。
