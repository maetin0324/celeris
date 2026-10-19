---
title: "Jev 副比較（B1/C1）の既存準備の棚卸し（再利用・段階別の欄・variant 固定位置・不足）"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: inventory
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# Jev 副比較（B1/C1）の棚卸し

基準: main c9654bff（本 worktree の base）。コードは読んだだけで変えていない。本番 DB/KB・本番 host には触れていない。試験は今回走らせていない（名前は静的に読んだもの。通過の証拠は各 progress の記録による）。
注意: `confidence`（`classifier_confidence`）は分類器自身の確信であり、成功確率でも propensity でもない（`crates/task-core/src/decision_log.rs:27`、試験 `decision_log_export_tests.rs: confidence_is_never_propensity_and_interventions_are_classified`）。以下の集計に confidence を成功確率として使わない。
用語: 現行コードの arm 名は A/B/C（`ConfigId`）のみ。B1/C1 は依頼上の名で、コード上の B/C（B=fixed、C=decision_assisted）が対応する。

## (1) 再利用できる機能と証拠

| 機能 | 場所 | 証拠（commit / 試験） | 検証の水準 |
|---|---|---|---|
| Jev 分類器（HTTP クライアント、fallback 理由、breaker） | `crates/jev-classifier`、`config/jev.rs`（`[jev] enabled` 既定 false） | f8cd2d08（公式 decisions schema・4xx の status/code/message 記録） | 実 API の成功は未検証（`2026-10-15-mlsys-min/live-smoke.md` 再実行 6: 観測は `not_attempted` のみ） |
| 分類段（dispatcher の外で Jev を呼び board に置く） | `crates/celeris/src/daemon/classify_stage.rs`、`classify_tick.rs` | classify_stage 試験 `success_is_jev_ok_with_a_hint_and_never_marks_the_real_api_verified` / `fixed_mode_never_calls_the_classifier_and_leaves_the_board_empty` / `fallback_records_fell_back_while_real_api_verified_stays_false` / `missing_key_hands_the_gate_to_the_existing_credential_wait` / `failures_become_fell_back_to_fixed_with_a_reason_name`；classify_tick 試験 `enabled_fills_the_board_for_a_ready_task_and_is_idempotent` / `only_the_runnable_work_unit_is_classified_by_its_unit_id` / `task_routing_override_beats_the_config_mode` / `credential_failures_are_reclassified_but_other_fallbacks_are_not` | fake 分類器（mock）。`classify_tick/e2e.rs` は fake Jev server で待ち→登録→一度だけ再開 |
| lane 層（分類で lane を引き上げ、明示・許可/天井・利用可否が勝つ） | `task-core/src/route_select/assist.rs`（`assist_lane`）、`task-dispatch/src/dispatcher/lane_assist.rs`（`apply_lane_assist`） | ab0818f1（basis 帰属修正）；`dispatcher/tests/lane_assist.rs`: `classification_only_raises_and_never_lowers_below_fixed` / `explicit_human_lane_beats_classification` / `injected_failures_fall_back_to_fixed_and_real_api_stays_false` / `dispatcher_and_store_never_reference_the_jev_classifier` | dispatcher 単体（mock）。`tests/mlsys_mock_e2e.rs`: `no_key_wait_register_resume_once_then_classify_run_log_export` / `classifier_failures_fall_back_to_the_fixed_lane_and_still_run_and_log` |
| task 単位の routing 固定（mode・下段・根拠表・online_update）と祖先継承 | `task-core/src/store/routing_overrides.rs`、`route_select/task_override.rs`、`campaign/src/celeris_exec.rs::routing_override` | 85fae6d7, 453abcff, 9c4af7f9；`routing_overrides.rs` 試験 `routing_override_is_inherited_by_descendants_and_nearest_wins` / `evidence_mode_none_survives_child_grandchild_and_retry_resolution`；dispatcher 試験 `task_fixed_beats_config_decision_assisted_and_no_assist_runs` / `task_decision_assisted_beats_config_fixed_and_assist_runs` / `root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config` | 単体・mock。実 daemon の子 run basis は未確認（`2026-10-16-mlsys-audit-fix-3-routing-inheritance.md` 提案） |
| Jev 呼び出しの消費記録（call_id 単位、費用欠測は NULL、再試行=`call_no>1`/`is_retry`） | `lane_assist.rs::record_jev_usage`、`decision_log.rs::DecisionLogUsage::jev_call` | b4552084；`jev_classification_cost_is_metered_once_per_campaign` / `jev_calls_are_kept_per_call_and_missing_cost_stays_null` | temp DB |
| decision log への run 投影（提案/決定 lane、候補、除外、exec model、fallback_reason、attempts、run_end、wu_verdict、wall_ms） | `dispatcher/decision_log_wire.rs`（`from_routing_record`・`fill_proposal`・`fill_execution`）、`store/decision_log.rs` | ad25ab34 ほか；`store/decision_log_tests.rs` | temp DB |
| campaign export / 集計（by_role に jev、by_retry、by_arm、runs_by_end、wu/campaign_verdicts、exec_vs_proposed、interventions_by_kind、incomplete） | `task-core/src/decision_log_export.rs`（`CampaignSummary`）、`celerisctl campaign` | 64c0afe7；`decision_log_export_tests.rs`: `other_campaign_rows_do_not_mix` / `reprocessing_does_not_double_count_and_output_is_byte_identical` / `missing_is_null_estimated_is_flagged_and_categories_stay_separate` / `failures_stay_in_denominator_and_labels_are_separate` / `retry_and_role_tables_are_separate_and_jev_is_distinct`；`celerisctl/tests/campaign_export.rs`, `campaign_connected.rs` | temp DB・fixture |
| Jev smoke 判定器（jev_ok/fallback/not_attempted/missing の区別、費用・版、許可内 lane、basis 帰属） | `crates/campaign/src/jev_smoke.rs`（`judge`/`check_smoke`） | 986b7ed5；`campaign/tests/jev_smoke.rs`: `distinguishes_jev_ok_fallback_and_missing` / `real_api_verified_constant_is_not_evidence` / `missing_cost_or_version_fails` / `lane_must_be_within_allowed_range` / `raise_attributed_to_fixed_rule_is_checked_against_fixed_lane` | fixture。入力は呼び出し側が `Observation` に組む（記録から自動では読まない） |
| campaign 識別 / manifest hash の伝搬 | `campaign/src/manifest.rs`（`compute_sha256`）、`celeris_exec.rs:327`（`campaign.manifest_hash`）、`decision_log.rs::CampaignIdentity` | `campaign/tests/celeris_exec.rs`、`routing_pin.rs::each_task_carries_its_arm_routing_and_the_record_matches_the_manifest` | 偽 task-api |
| B/C 比較条件の整合検査（lower_policy・models・run_harnesses 同一、A⊂B/C） | `manifest.rs::check_comparison_conditions`（final のみ） | `routing_pin.rs`、`harness_exec.rs` | 単体 |
| 実測の照合（別 model/harness、online_update=true、config への fallback を違反） | `campaign/src/checker.rs::reconcile_observed` | `routing_pin.rs::checker_flags_routing_that_differs_from_the_manifest` | 単体 |
| B/C 実行器（通常 POST /tasks、待機、子追跡、cancel）、template の tier/assignee 禁止 | `celeris_exec.rs`（`check_template`）、`curl_transport.rs` | `campaign/tests/celeris_exec.rs`、`mock_e2e.rs` | 実 HTTP 往復なし |
| 試行隔離・budget・final_accounting・grader・K2/K4・TrialRecord.wall_secs | `campaign/src/*` | 各 progress（`2026-10-16-mlsys-*`） | 依頼文の再利用対象。本棚卸しの範囲外 |
| 判定器の実 daemon 検証 | — | なし | **未実施**（`final-connect/final.md` 31 行） |

## (2) 段階別集計: 記録から読める欄 / 読めない欄

読む場所の略称: **RD**=`routing_decided` event の `record.decision.lane_assist`（`LaneAssistRecord`、`assist.rs:51`）、**DL-run**=decision log の run 行（`DecisionLogProposal`/`Execution`/`Outcome`）、**DL-use**=decision log の消費行、**Sum**=`CampaignSummary`。

| 段 | 読める欄（根拠） | 読めない／不確かな欄 |
|---|---|---|
| ① 分類試行 | RD `classifier_status.status` = `jev_ok`/`fell_back_to_fixed{reason}`/`not_attempted`（`assist.rs:20`）。`jev_smoke::classifier` が 4 値（+Missing）に分ける。DL-use の Jev 呼び出し行（`jev_call`、call_no、is_retry。`NotAttempted` は行を作らない `lane_assist.rs:record_jev_usage`）。Sum.by_role["jev"] | 「分類した件数」の分母は `routing_decided` の数のみ。**DL-run の `classifier`（kind/model/snapshot/confidence）を書く本番コードが無い**（`ClassifierRecord` の構築は試験のみ。`decision_log_wire.rs:409` は既存値の保持だけ）。export の `classifier_kind` は常に null になる。tick と dispatch の競合で `not_attempted` になる（live-smoke 再実行 6）ので「Jev 不調」と区別できない（reason 無し） |
| ② jev_ok | RD status、DL-use outcome（`ok`/`fallback:<reason>` が鍵に入る）、`jev_smoke` の `JevOk` | Jev の **版**: `ClassificationEntry.model_snapshot`（`assist.rs:35`）は board にあるだけで RD にも DL-use にも書かれない（jev-live.md 提案 2）。`classify_tick.rs` の snapshot は試験で None。→ `Verdict.version_recorded` は実記録から立たず、`check_smoke` は常に失敗 |
| ③ lane 提案変更 | RD `base_lane`/`proposed_lane`/`final_lane`/`basis`（`classification_raise` が Jev 帰属）。`jev_smoke::attribution` が整合を検算 | **`fixed_lane`（fixed 規則だけの lane）と Jev 自身の提案 lane が無い**。`proposed_lane` は採用された 1 つ。Jev が同意しても fixed が先に届く場合 Jev の寄与は basis に出ない（jev-live.md）。→ fixed_rule 引き上げは `Attribution::Unverifiable`。分類 hint（specificity）と confidence も RD に無い（board のみ）。DL-run の `proposed_lane`/`decided_lane` は `LaneDecision.proposed/lane`（既存方策）由来で lane_assist とは別（`decision_log.rs:from_routing_record`）。`lane_assist` 適用後の lane が `decided_lane` に入るかは未確認 |
| ④ 制約適用後 lane | RD `final_lane`、`overridden_by` = `explicit`/`ceiling`/`availability`（`assist.rs:68`）。許可内か: `jev_smoke::judge` は `allowed`/`max_lane` を**呼び出し側入力**で受ける | 許可 lane・天井・利用可否で閉じた lane の一覧は RD に無い（`LaneLimits` は記録されない）。`overridden_by` は 1 値のみで、複数制約が同時に効いた場合や「提案が無かったので抑止不要」と区別できない。lane 引き上げ前の `base_lane` が既に天井で切れていた場合も不明 |
| ⑤ 実行 model 変更 | DL-run `proposed_first`(source,model,harness) vs `exec_source_id`/`exec_model`/`exec_harness`/`exec_adapter`、`attempts`、`fallback_reason`（`decision_log_wire.rs:fill_execution`）。Sum.exec_vs_proposed（提案先頭と違う run 数、`exec_unknown` 別） | 「lane が変わったから model が変わった」の連結（run 単位で RD の lane_assist と exec_model を結ぶ集計）は無い。Sum.exec_vs_proposed は lane 変更の有無で層別しない。model の**版**（snapshot 文字列）は `provider_model_version` があるが埋め元は未確認 |
| ⑥ fallback・上限・明示指定による抑止 | fallback: RD `fell_back_to_fixed{reason}`（理由名）、DL-run `fallback_reason`（proxy が次候補へ送った理由 / 提案先頭と実行の差）。明示: RD `overridden_by=explicit`（`basis=fixed_fallback`）。上限: `overridden_by=ceiling`。budget 上限: manifest `stop_on_budget`、`campaign` の budget 記録・`max_overshoot_*`。利用不可: `availability` | 抑止理由を 1 欄に正規化した集計は無い（RD の 3 値＋classifier reason＋DL-run fallback_reason が別欄）。`credential` 待ちで dispatch が止まった時間・回数は RD に出ない（別 event）。budget 由来の抑止は lane_assist と別系統（campaign 側） |
| ⑦ 最終成果・再試行・全費用 | DL-run outcome `run_end`、`wu_verdict`、`campaign_verdict`（3 階層は別表）、`wall_ms`。再試行: `parent_run_id`/`attempt_no`、Sum.by_retry。費用: DL-use（worker/reviewer/planner/jev/cos、`cash_cost_usd` 欠測は null、`list_cost_usd` は推定）、Sum.total/by_role/by_arm/shared_overhead/incomplete。`TrialRecord.wall_secs`、`final_accounting.rs` | 実支出（cash）は取れず定価換算の推定のみ。Jev の token 数、CoS の turn 単位 usage、`decision_log_interventions` の書き手が無い（`final-connect/final.md` 42 行）。`quota` は常に null |
| 横断: 段の連結 | `task_id`/`work_unit_id`/`run_id`/`decision_id` で RD と DL を結べる | 上記 ①〜⑤ を **1 行に並べる段階別集計器が無い**（`jev_smoke` は ①②③ の一部のみ、`CampaignSummary` は ⑤⑦ のみ）。B/C を同じ表に並べる層別（arm × 段）も無い |

## (3) B1/C1 variant の固定位置と子/孫/retry への継承経路

固定されるもの（現行 A/B/C の manifest 上の位置）:

| 項目 | 固定位置 | 継承・検査 |
|---|---|---|
| manifest hash | `CampaignManifest.sha256`（`manifest.rs:270`、`compute_sha256`）。`validate` が内容と一致検査。task 作成時に `campaign.manifest_hash` へ（`celeris_exec.rs:327`）、DL に `manifest_hash`（`CampaignIdentity`）、ledger key は `manifest_sha256`+problem+arm+trial | 台帳 key が hash を含むので別 hash の行は混ざらない（`final_accounting.rs:376`） |
| 方策差（唯一の差であるべき点） | `Config.id`（B=fixed / C=decision_assisted）、`policy_version`（C は Jev prompt/model 版を含む）。`routing_override.mode` は arm で決まる（`celeris_exec.rs:routing_override`） | root task の `task_routing_overrides`（migration 0084）→ 祖先鎖を辿り最も近い祖先が勝つ（`routing_overrides.rs`）。子・孫・retry は試験 `root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config` で確認（mock） |
| 候補集合（Sonnet/Opus） | `ArmRouting.models`（B/C 同一を `check_comparison_conditions` が要求）。**宣言は任意**で、検査は事後（実測 `ObservedRun` が集合内か） | 選択時に強制する機構は無い。final.json の B/C は `models` 未宣言で `model=claude-sonnet-5-5` 単独（Opus 候補を含まない） |
| 階層対応（lane→model） | `ArmRouting.lower_policy`（既定 `mlsys-min-1`、`task_override.rs` が既知版のみ受理）。B/C 同一を検査 | task 欄で固定。子へ祖先鎖で継承 |
| harness | `Config.harness`/`harness_version`（celeris は 12 hex の release sha）、`ArmRouting.run_harnesses`（B/C 同一）、能力 probe `Config::worker_harnesses` | final で `check_resolved`。実行側は `ObservedRun` で事後照合 |
| 予算 | `Budget`（max_tokens / per_case / per_arm / max_wall_secs / max_parallel）、`Termination`（`max_attempts_per_trial=1`）。final.json: 全体 300 万 / per_case 30 万 / per_arm 130 万 / timeout 1800s | campaign 全体で共有。**per_arm は arm ごと**なので B と C は別枠だが同額 |
| 初期情報 | `InputRef.sha256`+`InputManifest`、`SnapshotRef`（repo_commit / materials_sha256 / kb_snapshot）、`task-template.json`（title/objective/workspace/max_wall_secs/routing_override/assignee/tier は template で禁止＝実行器が埋める） | 試行隔離（`trial-context`）。arm 間で同一 problem・同一 input |
| grader | `grader.rs`/`grade_runner.rs`、独立採点（audit-fix-1 `grader-binding`、`grade_binding.rs`） | arm を見ずに採点 |
| 根拠表を使わない | `routing.no_evidence_table=true`（C のみ）→ `evidence_mode=none` | `2026-10-18-resolved-routing-evidence-mode-default.md`、`evidence_mode_none_survives_child_grandchild_and_retry_resolution` |
| C が単一 model/lane に pin されていない | `check_template` が `tier`/`assignee`/`routing_override` を禁止 | **Jev の lane 提案が「効く余地」が残る構成かを検査する機構は無い**（下記不足） |

## (4) 不足一覧

重大度: **H**=C1 の段階別評価に必須、**M**=副比較の解釈に必要、**L**=改善。いずれも主比較（A0/B0）の依存先ではない。

1. **H** `LaneAssistRecord` に `fixed_lane`・`jev_lane`（Jev の提案 lane）・Jev 版（`model_snapshot`）が無い。→ ②の版が常に欠測、③の fixed_rule 引き上げは `Unverifiable`（jev-live.md 提案 1〜3）。本 task の stage-agg/synth-e2e が前提にする。
2. **H** DL-run の `classifier`（kind/model/snapshot/confidence/fallback）を書く本番経路が無い。export の `classifier_kind` は null。
3. **H** 段①〜⑦を 1 行にまとめる段階別集計器が無い（arm×段）。`jev_smoke` は実記録から `Observation` を組まない（`allowed`/`max_lane`/`usage_present`/`cost_usd` を呼び出し側が渡す）。
4. **H** B1/C1 を**別の実験セット・別測定 ID**として宣言する manifest 形が無い。`validate` は configs が A,B,C ちょうど、`primary==(A,B)`、`secondary.pair==(B,C)` を要求する（`manifest.rs:validate`）。A を含まない B1/C1 manifest はそのままでは通らず、B1/C1 を A0/B0 の manifest に同居させると主比較の hash に Jev 設定が入る（依存先になる）。
5. **M** 候補集合（Sonnet/Opus）を B/C で同一に宣言する運用が final.json に無い（`routing.models` 未宣言＝照合が効かない。`model` は Sonnet 単独）。選択時の強制は無く事後検査のみ。
6. **M** 「C が lane 提案を出せる構成（許可 lane・天井が Opus lane を塞いでいない）」を検査する機構が無い。`overridden_by=ceiling`/`availability` が多発すると C1 は B1 と同じになる。
7. **M** 子/孫/retry の継承は mock 試験のみ。実 daemon で子 run の `routing_resolved`・`lane_assist` が親の pin を維持した実ログは無い（audit-fix-3 提案）。
8. **M** 実 Jev 呼び出しの成功は未検証（live-smoke 再実行 6）。分類が `not_attempted` になる tick/dispatch 競合と、Jev 不調の fallback を区別する reason が無い。
9. **M** 抑止理由の正規化（explicit / ceiling / availability / classifier fallback / budget / credential 待ち）が複数欄に分散。`overridden_by` は 1 値。許可 lane・天井が RD に無い。
10. **M** lane 変更を exec model 変更へ連結する集計が無い（`exec_vs_proposed` は lane で層別しない）。
11. **L** 費用は定価換算の推定のみ（cash 欠測）。Jev token 数・CoS turn usage・interventions の書き手が無い（final.md）。
12. **L** `ArmDispatch` は `usage` を転送せず実行中の上限判定が効かない（audit-fix-3 提案）。

## 次の WorkUnit との対応（参考）
- stage-agg: 不足 1〜3, 9, 10（campaign crate の集計。本体記録の欠けは「読めない」として欠測のまま出し、推測で埋めない）。
- variant-pin: 不足 4〜7。
- synth-e2e: 合成入力で RD→DL まで届く通常経路（不足 2, 10 が露出する見込み）。
- live-runbook: 不足 8（実 daemon・認証は人の手順）。
