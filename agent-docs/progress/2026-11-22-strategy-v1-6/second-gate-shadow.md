---
title: 戦略層 v1 子 6 second-gate-shadow — 第二ゲートの shadow ケース（アーキv4 replay・反証・予算・停滞・負の研究結果・回帰）
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: second-gate-shadow
status: done
updated: 2026-11-23
---
# second-gate-shadow（WorkUnit）

完了日: 2026-11-23

設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D8・D9・D11 と末尾の付記（WU second-gate-shadow）。
基準はこの WU の `$CELERIS_WU_BASE`。production のコードは 1 行も変えていない（試験・fixture・ADR の付記・この進捗だけ）。

## 実装前 brief

1. 根本原因: 子 6 の部品（snapshot の checkpoint・検証・写し・評価の決着）は WU ごとの試験はあるが、子 0 のアーキv4 の replay の時系列を流して
   「途中で発火し、評価が pivot・人への判断に写る」ことを一続きで示す試験が無かった（第二ゲートの証拠が無い）。
2. 責務 owner: 試験だけ。owner の関数は `task_ops::project_snapshot::{build, checkpoints}`・`task_core::project_proposal::parse_and_validate`・
   `task_core::parent_checkpoint::{check_evaluation, ParentEvaluationRecord, allowed_actions}`・`task_worker::change_rules::review::parent_checkpoint_section`。
3. 既存の再利用先: 子 0 の `replay.py build` の slice と `manifest.sha256`、子 2 の戦略の文書の fixture（`task-core/src/strategy/fixtures/baseline-v1.json`）、
   `crates/celeris/tests/project_controller_e2e.rs` の harness の形（判断の記録と revision を harness が持つ）、task-dispatch の試験の strategy root の作り方。
4. 選択肢: (a) task-api に task-dispatch を dev-dependency で足し本物の tick を回す → Cargo.toml は WU の範囲の外で、deno_core の build も重い。
   (b) 試験を task-dispatch に置く → D11 と WU の範囲が task-api を指定。(c) task-api で snapshot・検証・決着の本物の関数を通し、TS の規則は D5.1 を写した試験の中の規則にする。(c) を採った。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 無し（試験の追加）。
7. 不変条件と試験: 下の表。

## 試験（すべて task-api。LLM・外部ネットワークなし）

| 試験 | 守る不変条件 | 結果 |
|---|---|---|
| `parent_checkpoint_shadow_fixture_matches_manifest` | 12 slice の `slice.json` と cutoff で切った events・E-mid の `goal.md` が子 0 の manifest の sha256 と一致し、fixture に answer key の file・印が無い | pass |
| `parent_checkpoint_shadow_fires_at_c_mid_d_done_f_done` | shadow の 12 時点で `children:2`（C-mid）・`children:4`（D-done）・`children:6`（F-done）だけ発火、`suppressed` は発火済みで 2 回目が無い | pass |
| `parent_checkpoint_shadow_enabled_replay_maps_c_mid_continue_d_done_pivot` | enabled の replay で C-mid は continue、D-done は `replan{checkpoint_pivot}`、F-done は `escalate(checkpoint:<id>)`。評価の run は 3、task の数は不変、D-done は負の研究結果 | pass |
| `parent_checkpoint_shadow_no_progress_escalates_while_unmapped` | 写していない評価の間、`pc/no-progress` の escalate と paused の wait は通り、launch_root・表に無い continue は `checkpoint_action_mismatch`。guard の後に act が写す | pass |
| `parent_checkpoint_shadow_continue_mapping_unblocks_next_launch` | continue の写し（`none`）は写し済みで、次の起票（launch_root）は checkpoint で止まらない。写す前は止まる | pass |
| `parent_checkpoint_shadow_mode_raised_to_enabled_evaluates_next_due_once` | shadow の `suppressed`（children:2）の後に enabled に上げると、次の due（children:4）で評価の run が 1 回、children:2 は評価し直さない | pass |
| `parent_checkpoint_shadow_budget_50_80`・`…_child_failed`・`…_refute_in_subtree` | D8.4 の合成: 予算 50・80 %、子の failed、subtree の新しい版の反証で due | pass |
| `parent_checkpoint_shadow_unsupported_claims_never_on_track` | url だけ・参照 0 件の主張は unsupported（2）で、on_track・goal met にならず next は ask_human、写しは escalate だけ | pass |
| `parent_checkpoint_shadow_suppressed_writes_no_evaluation` | shadow の評価は記録の成果物を書かない | pass |
| `parent_checkpoint_regression_no_strategy_root_no_reviewer_run` | strategy 無しの root（子 4 件 done）で checkpoint 無し・snapshot の JSON に `checkpoints` の key 無し・`evaluate` は `checkpoint_root_unknown`・reviewer run 0 | pass |
| `parent_checkpoint_regression_controller_off_no_reviewer_run` | 行の無い案件と `off` の案件は `project_controller_active` に入らず、判断 0・reviewer run 0 | pass |
| `parent_checkpoint_regression_shadow_no_reviewer_run` | shadow の `evaluate` は `suppressed`、2 回目は `checkpoint_not_due`、reviewer run 0 | pass |
| `parent_checkpoint_regression_final_review_unchanged` | 節の無い review の `request.json` に `parent_checkpoint` の key が無く、prompt の節は空 | pass |
| `parent_checkpoint_live_request_renders` | E-mid の slice から作った request と prompt が commit した `E-mid.request.json`・`E-mid.prompt.md` と一致し、answer key の印と E-mid の理由の文を含まない | pass |
| `parent_checkpoint_live_sample_checks`（`#[ignore]`） | live-evaluator-sample の評価の検査（その WU が走らせる） | 未実行（評価の JSON は未作成） |

既存の試験で検出できない理由は各 file の先頭の doc comment に書いた（task-core・task-dispatch の試験は合成の事実で、replay の時系列を流さない）。

## 測定（試験の出力）

- 発火の時点（shadow、N = 2）: C-mid（cutoff 2026-11-18T07:41:56Z、`children:2`）・D-done（2026-11-18T20:39:40Z、`children:4`）・F-done（2026-11-19T04:55:10Z、`children:6`）。
  他の 9 時点は発火しない。ADR D8.1 の表と一致。
- 史実の親の最初の評価（2026-11-20T14:12:10Z）との差: C-mid 54.5 時間、D-done 41.5 時間（`d_done_lead_hours=41.54`）、F-done 33.3 時間早い。
- 起きた pivot: enabled の replay で D-done の評価（off_track・pivot）が `replan{reason_code: checkpoint_pivot}`（context_refs は root・最終の記録・戦略の文書だけ）に写った。
  F-done（off_track・ask_human）は `escalate`（question_key `checkpoint:dec-005`）。
- 評価の回数: 発火 3 回 = 評価の reviewer run 3 回（スクリプトの評価）。shadow では 0 回。回帰の 3 件（strategy 無し・controller off・shadow）で reviewer run 0。
- answer key との比較（出力だけ）: C-mid at_risk/continue、D-done off_track/pivot、F-done off_track/ask_human で 3 点とも一致。ただし評価器の出力は
  answer key に合わせて書いたスクリプトなので、一致は写しの経路の確認で、評価の質の根拠ではない（評価の質は live-evaluator-sample と子 8）。
- live の request: E-mid で終端の子 3/8、request 4,264 byte、prompt 3,747 byte。
- LoC: production 0 行。試験 1,620 行（shadow 1,015・regression 314・live 291）。fixture 376 KB（G-done の events 296 KB と 12 の slice.json・goal.md・スクリプトの評価、live の 2 file）。

## checkpoint の効果を外したときの失敗（試験の file を一時的に変えて実行し、元に戻した）

- M1: harness の snapshot の `checkpoints` を空にした（checkpoint の節の無い snapshot = 変更前の controller と同じ）。
  `cargo nextest run -p task-api --test parent_checkpoint_shadow --no-fail-fast -E 'test(/parent_checkpoint_shadow_/)'` → exit 100、11 件中 10 件 fail（pass は fixture の検査 1 件）。
- M2: enabled の `evaluate` の効果（評価の run と最終の記録）を外した。同じ command → exit 100、11 件中 5 件 fail（enabled_replay・no_progress・continue_mapping・mode_raised・unsupported）。
- 戻した後は 11 件 pass。

## 証拠（提出時の HEAD で実行）

（検査の結果は下の「検査」に書く）

## 未解決事項

- D-done の pivot から子 5 の gate を通る新しい版の起票と、却下した候補に戻る版の `strategy_revisit_rejected` は、この WU の試験では確かめていない（task-api の
  `/api/v1/cos/operations` を通す子 5 の shadow の流れ〈`strategy_review_shadow.rs`〉は E-mid で同じ gate を確かめている）。replan の写しが CoS 入力になる所までが確認済み。
- discuss-only の thread から task を作れないことはこの WU では試していない（子 3 の試験の範囲）。評価の出力が task を作れないことは、enabled の replay で task の数が不変で、
  写しの表に起票の行動が無いことで確かめた。
- task-api の試験は TS の host を通さない（ADR の付記）。TS の規則の順は control-ts の試験と celeris の e2e が確かめる。
- live の request は dispatcher の private の `request` を呼べないので同じ出どころの欄を試験が組み立てる。dispatcher 側の組み立てとの byte の一致は未確認。

## 提案

- 子 6 の close-gate で、D-done の pivot から子 5 の gate を通る版までを 1 つの試験でつなぐか（task-api の子 5 の shadow の helper を共有する）を決める。
