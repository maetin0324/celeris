---
title: "少数 pilot と評価条件の固定（mock 実行・見積り・未検証の区別）"
unit: pilot
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# pilot

## 結論
- pilot（split `dev`、A/B 各 5 問、C は事前 subset 2 問）を `celeris-campaign` の dry-run → mock run → check → check-report → export で通した。全て exit 0。
- **実 LLM 測定は行っていない**。実 Celeris 実行器が未実装（`Executor` は mock のみ。`main.rs` は `--executor mock` 以外を拒否）で、実測の予算決定（measure）も未了。新しい有料測定は始めていない。キー値は扱っていない。

## 固定した評価条件（pilot）
| 項目 | 値 |
|---|---|
| campaign_id / version | `pilot-dev-1` / `1` |
| campaign sha256 | `c7ccacb8654967f3b26c517b6a86d9f87d024af7ed793b7716b18f15200ea6be` |
| input manifest 参照 sha256 | `52dd6e1b3544b47a0987a4ac331dee4b041d2ee5ba41ab32e772e1b588479dd5`（manifest 内の宣言値。`validate`/`dry-run` が照合し一致） |
| split | `dev`（入力 5 問は全て split=dev。final は `held-out` 名で別 manifest） |
| 問題 | impl-benchfs-chunk-index / exp-benchfs-smallrun / analysis-benchfs-scaling / report-pluvio-summary / review-numeric-consistency |
| A | harness claude-code `baseline-1`、model `mock-model` `mock-1`、policy `ops-baseline` |
| B | harness celeris `1`、model `mock-model` `mock-1`、policy `fixed` |
| C | harness celeris `1`、model `mock-model` `mock-1`、policy `jev-1`（B/C は subset 2 問のみ） |
| 主比較 / 副比較 | A vs B（5 問）/ B vs C（impl-benchfs-chunk-index, review-numeric-consistency） |
| 予算 | max_tokens 1,000,000 / wall 36,000 s / disk 128 GiB / 並列 2 |
| 終了条件 | trial timeout 1,800 s、max_attempts 1、`stop_on_budget` true（cancelled は分母に残す） |
| snapshot | repo `fixture-repo-1`、materials sha256 `692578…f9581`、KB `kb-fixture-1` |

注: モデルは mock 名。実測へ進む際は model / version を実名で固定した**新しい** manifest（新 hash）を作る。

## pilot と最終評価を混ぜない規則
- final は `split: final`（入力側の名前 `held-out`）の別 manifest・別 campaign_id・別 sha256 で作る。pilot の `dev` 問題を final に入れない。
- checker/runner は campaign split と入力問題 split の不一致、manifest hash の不一致を拒否する（`manifest.rs` の `split_mixing_is_rejected` ほか、`runner_checker` の改ざん試験）。
- pilot の結果を較正（見積り係数の更新）以外に final の主張へ使わない。

## 見積り（dry-run、estimated。`llm_calls_made 0`）
- tokens: A 250,165 / B 250,165 / C 103,068、合計 **603,398**（上限 1,000,000 内）。
- 資源: 試行 12、並列 2、wall **3,600 s**、disk **4 GiB**。`within_budget true`、`reasons []`。
- 係数は仮置き（chars_per_token 4、context 30,000/試行、output 20,000/試行、Jev 1,500/decision、600 s/試行、2 GiB/試行）。実測で較正が必要。

## mock pilot の結果（実性能ではない）
- run: `executed 12, resumed_skipped 0, budget_cancelled 0, blocked 0, waiting 0, tokens_total 113000, budget_stopped false`。
- check: `ok true`、violations 0、records 12。check-report（report-good.md）: `{"ok":true}`。export: `table.json/csv`（`celeris.campaign-table/1`）。
- 主比較 A vs B: both_pass 3、only_A 2、only_B 0。B/C 副比較: both_pass 2。A は 5/5、B は failed 1・error 1 を含む（mock の seeded 結果。失敗は分母に残る）。
- 5 問・1 試行なので統計的主張はできない。

## 実行したコマンド（`B=$CARGO_TARGET_DIR/debug/celeris-campaign`、`F=crates/campaign/fixtures`）
`$B dry-run|validate --manifest $F/pilot.json`（exit 0）、`$B run --executor mock … --out $TMPDIR/out`（exit 0）、`$B check`（0）、`$B check-report --report $F/review/report-good.md`（0）、`$B export`（0）。

## 実装済み / 未検証
実装済み・mock 上で試験済み: manifest 検証と hash/split 固定、dry-run 見積り、runner（隔離・再開・予算停止・依存待ち）、独立 checker、export、check-report。
未検証・未実施: 実 LLM での pilot/最終測定、実 Celeris 実行器（task 作成と RunMetrics からの token/cost 採取）、見積り係数の較正、Jev 実 API 分類の成否（live-smoke は client_error fallback 観測のみ、成功は未観測）、統計的有意性。

## 提案
- measure の前に人が予算を決める（measure-budget）。まず A/B 各 1〜2 問の実測で係数を較正し、実行器（Executor の Celeris 実装）を作る。
- 実測用 manifest は model 実名で新 hash、final は held-out 別 manifest にする。
