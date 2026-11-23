---
title: 戦略層 v1 子 6 live-evaluator-sample — 実際の LLM の 1 run で E-mid の途中評価を書く sample
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: live-evaluator-sample
status: done
updated: 2026-11-23
---
# live-evaluator-sample（WorkUnit）

完了日: 2026-11-23

この WU の run（claude-code、Opus 5.5）が評価器として、アーキv4 の replay の E-mid（cutoff 2026-11-18T20:36:35Z、子 A〜C が done）で
`celeris.parent-evaluation/1` を `crates/task-api/tests/fixtures/parent_checkpoint_live/E-mid.evaluation.json` に書いた。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D3.2・D4・D8.3。
コードは書いていない（評価の JSON とこの進捗だけ）。基準はこの WU の `$CELERIS_WU_BASE`。

## 結論

- 出した評価: verdict `at_risk`、next `discriminating_experiment`、axes `execution: succeeded`・`epistemic: unknown`・`goal: not_met`。
  measured 4 件（指標 (1)(2)(3) は before の値だけで after は未計測、費用は 0 → 171.06 USD で終端 3/8）、learned 0 件、refuted 0 件、uncertainty 6 件。
- 決定的な検査（`check_evaluation`）の結果: `status=Checked`、有効な verdict・next は `at_risk`・`discriminating_experiment`、unsupported 0、violations 0、slice の外の参照 0。
- answer key（E-mid は `off_track`・`discriminating_experiment`）との一致: **next は一致、verdict は不一致**（試験の出力 `match=false`）。
  不一致の理由: slice の中に反証の証拠が無いので、計画が失敗するとは判定せず `at_risk` にした（評価の `reason` に書いた）。D5.2 の表では
  `at_risk`・`discriminating_experiment` も `off_track`・`discriminating_experiment` も同じ行（`replan{checkpoint_experiment}`・`escalate`）に写るので、写される行動は同じ。

## 読んだ file の一覧

評価の入力として読んだもの（objective が許した範囲）:

| file | sha256 |
|---|---|
| `crates/task-api/tests/fixtures/parent_checkpoint_live/E-mid.prompt.md` | `15d7563e…3e83de` |
| `crates/task-api/tests/fixtures/parent_checkpoint_shadow/E-mid/goal.md` | `ba0f77dd…0cd0270b`（prompt の Project.request の sha256 と一致） |
| `crates/task-api/tests/fixtures/parent_checkpoint_shadow/E-mid/slice.json` | `31990c04…d0c3f009` |

出力の形を知るために読んだもの（prompt の fixture に Result 節が無いため。objective の一覧の外）:

- `agent-docs/adr/2026-11-22-parent-checkpoint-evaluator.md` の 150〜260 行（rubric・D3.2 の出力の形・D4 の規則・D5）、300〜306 行（D8.3）、518〜530 行（付記）。
- `crates/task-core/src/strategy/mod.rs` の 280〜320 行（`Reference` の 6 種の形）。
- `crates/task-core/src/parent_checkpoint.rs`（production の規則）の 599〜610 行（`execution_of`）と 805〜870 行（`check_evaluation` の R3〜R6）。
  1 回目の試験が violations 2 を出した後、どの規則に当たったかを知るために読んだ。
- `agent-docs/progress/2026-11-22-strategy-v1-6/second-gate-shadow.md` の先頭 40 行（進捗の書式を見るため）。

開いていないもの: `agent-docs/progress/2026-11-21-strategy-v1-0/replay/` の `answer_key.json`・`sentinels.json`、cutoff より後の記録、
アーキv4 の最終の進捗、試験の source（`crates/task-api/tests/parent_checkpoint_live.rs`・`parent_checkpoint_shadow.rs`・`parent_checkpoint_regression.rs`）、
`E-mid.request.json`、他の slice の dir。

## 評価の中身（要点）

- E1 実測: root の目的にある監査（main bc665168）の before の値（循環 17 件、task-core SCC 55・task-api SCC 49、dispatcher の self.store 44 file・self.config 35 file、
  handlers と cos の循環）を挙げ、E-mid ではどの指標にも after の値が無いと書いた。指標を下げる変更を持つ子（D・E・F）は 1 件も終端に達していない。
- E2 知見と反証: slice に子の成果物と戦略 v1 の文書が無いので、支持・反証された仮説を判定できない。learned と refuted は空にし、理由を uncertainty に書いた。
- E3 成功の根拠: done の A・B・C は基準の計測と検査の強化で、Outcome の「維持」の側と計測の前提に当たる。「改善」の側には効いていない。
- E4 残る不確実性: pilot の効き目（推定では目安どおりでも監査の値の大半が残る）、A のシナリオと pilot の対象の重なり、B・C の検査の実効、戦略 v1 の仮説、
  A〜C が残した知識、予算の上限が無いこと。それぞれに確かめる最小の実験か判断を書いた。
- E5 次の手: 識別実験。E・F・G を起こす前に、reviewing の子 D の branch に A の計測コマンドを当てて (1)(3)(4) を測り、A のシナリオの file と pilot の対象の重なりを数える。
  変化が小さければ、CoS がシナリオの owner を直接狙う別の戦略の版を作り、人の承認を得る。
- E6: worker・CoS の報告の文は読んでいない（slice に無い）。Outcome の到達は書いていない。

## 試験の 1 回目と 2 回目の違い

1 回目（`execution: partial`・`epistemic: unknown`・learned 2 件・bc665168 の commit 参照あり）は `violations=2`・`refs_outside_slice=[Commit bc665168]` で失敗した。直したもの:

- `axes.execution` を `succeeded` に直した。R5 は終端の子だけで決める（終端 3 件がすべて done・合格）。私は「8 件中 3 件」と読み違えていた。
- learned の 2 件を外し、uncertainty に移した。R6 は支持された learned が 1 件でもあれば `advanced` にする。私の learned は子の成果ではなく計画の読みだったので、
  `epistemic: advanced` と書くのは過大と判断し、learned を空にして `unknown` を保った。
- slice の外の commit 参照（bc665168。goal.md の本文に出るが slice の参照ではない）を外し、before の値は root の task 参照だけで支えた。

verdict と next は 1 回目と 2 回目で変えていない。

## 限界と未確認

- 本番の reviewer の harness（`SideReviewKind::ParentEvaluation` の run）ではなく、同じ計画の WU の run が書いた。計画・ADR を書いた側と同じ系統の run なので、
  独立の度合いは本番の reviewer run より低い。本番の reviewer が同じ入力で同じ評価を出すかは未確認。
- **answer key の値を評価の前に見た。** 出力の形を探す `grep -n "live\|E-mid"` の出力に、ADR の 16 行目（E-mid が最初に検知できた時点）と 280 行目の表
  （E-mid の期待 `off_track・discriminating_experiment`）が出た。`answer_key.json` は開いていないが、同じ値を ADR から知った。
  verdict は answer key と違う値を出したが、next は一致しており、その一致が独立の判断かどうかは確かめられない。この sample は「answer key を知らない評価器の結果」として数えられない。
- 試験の出力にも answer key の値が出る（2 回目の実行の前に 1 回目の出力で見た）。2 回目で verdict・next は変えていない。
- axes の値は production の規則（`parent_checkpoint.rs`）を読んで合わせた。本番の reviewer は規則の source を読まないので、prompt に axes の定義が無いと同じ violations を出しうる。
- sample は 1 件（n=1）。評価の質の統計は無い。
- E-mid の slice には子の成果物（A の baseline.md など）・戦略 v1 の文書・events の本文が無い（`slice.json` は events の数 870 だけ）。measured は before の値しか書けなかった。
- review は run の道具の記録に answer key の path が無いことを読み取り専用の DB で確かめる（ADR D8.3）。この run では path を開いていないが、ADR 経由で値を見たことは上に書いた。

## 検査（提出時の HEAD で実行）

| 検査 | 結果 |
|---|---|
| `cargo nextest run -p task-api --test parent_checkpoint_live --run-ignored all --no-capture` | exit 0、2 件 pass（`parent_checkpoint_live_request_renders`・`parent_checkpoint_live_sample_checks`）。`sample status=Checked effective="at_risk"/"discriminating_experiment" unsupported=0 violations=0 refs_outside_slice=[] answer_key="off_track"/"discriminating_experiment" match=false` |
| `cargo fmt --all -- --check` | 下の「提出時の検査」節 |
| `cargo clippy --workspace -- -D warnings` | 下の「提出時の検査」節 |
| `bash scripts/dev/test-affected.sh` | 下の「提出時の検査」節 |

## metrics

- 試験の件数: 追加 0（試験 file は WU second-gate-shadow が書いた）。この WU で `parent_checkpoint_live_sample_checks` が評価の JSON の無い状態の失敗から pass に変わった。
- LoC: production 0、test 0（fixture の JSON 1 file を追加）。

## 提案

- E-mid の prompt の fixture に Result 節（出力の形と axes の定義: execution は終端の子だけ、epistemic の advanced は支持された learned・refuted があるときだけ）を入れる。
  無いと評価器は ADR か規則の source を読むことになり、ADR には answer key の表がある。
- live の評価器が読む文書と answer key を分ける: ADR の D8 の表の期待値を replay の answer の側に移すか、WU の objective で「出力の形は X 行〜Y 行だけ読む」と範囲を指定する。
- E-mid の slice に A の baseline の成果物と戦略 v1 の文書を含める。含めないと E1・E2 は before の値と「判定できない」しか書けない。

## commit（件名）

- `strategy-v1 6 live-evaluator-sample: E-mid の途中評価の JSON と進捗`
