---
title: 戦略層 v1 子 6 checkpoint-design — 親の途中評価（checkpoint・Evaluator・pivot）の ADR
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: checkpoint-design
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# checkpoint-design（WorkUnit。設計だけでコードは書かない）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md)。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## 決めたこと（ADR の節）

- D1 発火条件: strategy root（CoS が `strategy` 付きで起票した root）だけ。trigger は子の終端の数（N 既定 2）・子の `failed`・subtree の反証の証拠・root の `max_tokens` の 50%・80%。
  発火・評価済み・写し済みは判断の表・events・`cos_operations` から毎回導き、保存しない。
- D2 `ProjectAction::Evaluate{root_id, triggers}`、検証の 4 code、効果は既存の reviewer run を 1 回（`start_design_review` と同じ起こし方）。
- D3 wire: `ReviewRequest.parent_checkpoint`・`ReviewOutput.parent_evaluation`（`celeris.parent-evaluation/1`）、固定の rubric E1〜E6、結果は root の `ArtifactProduced`（`parent-evaluation/<判断 id>.json`）。
- D4 決定的な規則 R1〜R7（参照の無い改善は unsupported で on_track にしない、goal の met は D6 だけ、execution は Rust の事実）。
- D5 next → continue・replan（`checkpoint_experiment`・`checkpoint_pivot`）・escalate の表と Rust の検証 `checkpoint_action_mismatch`。
- D6 評価は進捗に数えず、`runaway_guard` が `evaluate` も数える。D7 負の研究結果は 3 軸。
- D8 replay: N=2 で C-mid（B の done）・D-done・F-done で発火。最初の off_track の checkpoint は D-done（E-mid の 2 分 50 秒後）。answer key は比較の側だけが読む。
- D9 回帰の試験、D10 子 5 の穴は見送り（親の closeout で人が決める）、D11 WU の write set と試験の prefix。

## 検査（提出時の HEAD）

- `grep -c` で ADR の節「実装前 brief」「守る不変条件」「保証と実装の対応」「足さないもの」「未確認」が各 1 件以上あること、7 つの試験の prefix が ADR にあることを確かめた（報告に出力）。
- コード・schema・試験は変えていないので `test-affected.sh`・clippy・fmt の対象の変更は無い（`test-affected.sh --list` で確かめた）。

## 未解決

- ADR「未確認」の節のとおり（評価の質、反証の trigger の範囲、予算は `max_tokens` だけ、評価の run の間の root の継続の待ち）。

## 提案

- 子 5 の穴（選ぶ版を結果より先に書ける）を gate で閉じるか（版の `ArtifactProduced` の時刻と前の版の実験 task の終端の時刻の比較）は、親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の closeout で人に採否を聞く（ADR D10）。
- `cos_operations` を読む task-core の読み取り専用の関数（ADR D1.1 の `strategy_ref_of_task`）ができたら、子 5 の gate の生の SQL も同じ層へ移す（子 5 の進捗の提案と同じ条件）。
