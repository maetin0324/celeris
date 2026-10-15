---
title: "最小キャンペーン評価基盤: 統合進捗（実行手順・dry-run・見積り・比較計画・限界）"
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# campaign（統合進捗）

`crates/campaign`（`celeris-campaign`）の manifest / fixture / runner / checker / export / dry-run を mock 実行器の上で通した記録。個別の経緯は `campaign-manifest.md` / `campaign-runner.md` / `campaign-fixtures.md`、ADR は `agent-docs/adr/2026-10-15-mlsys-min-campaign.md`。実 LLM 測定はしていない。

## 1. 実行手順（実コマンド。2026-10-15 に実行し全て exit 0）
`B=$CARGO_TARGET_DIR/debug/celeris-campaign`（または `cargo run -p celeris-campaign --`）、`F=crates/campaign/fixtures`、`O=$TMPDIR/out`。
```
$B dry-run  --manifest $F/pilot.json                      # 見積り JSON。LLM 呼び出し 0
$B validate --manifest $F/pilot.json                      # campaign sha256 を表示
$B run --executor mock --manifest $F/pilot.json --out $O \
  --repo-src $F/repo --materials-src $F/materials --kb-src $F/kb
$B check --manifest $F/pilot.json --out $O                # 生記録から独立再計算
$B check-report --manifest $F/pilot.json --out $O --report $F/review/report-good.md
$B export --manifest $F/pilot.json --out $O               # checker 違反ゼロの時だけ table.json/csv
```
実行結果: validate = `c7ccacb8…ea6be`、run = `executed 12, resumed_skipped 0, budget_cancelled 0, tokens_total 113000, budget_stopped false`、check exit 0、check-report `{"ok":true}`、export exit 0（`celeris.campaign-table/1`、split dev）。

## 2. dry-run と見積りの読み方
pilot（dev split、A/B 各 5 問、C は事前 subset 2 問）の dry-run 出力の要点:
- `acceptance`: `manifest_valid true`、`input_hash_and_split_checked true`、`within_budget true`、`reasons []`、`label "estimated"`。`within_budget false` なら reasons に超過項目が出る。
- `tokens`（見積り、estimated）: A 250,165 / B 250,165 / C 103,068、合計 **603,398** token（予算上限 1,000,000）。内訳は input（問題文＋context overhead 30,000/試行）と output（20,000/試行、C は Jev 判定 1,500/decision を加算）。
- `resources`（見積り）: total_trials **12**、並列 2、wall **3,600 s**（600 s/試行 × 12 ÷ 2。上限 36,000 s）、disk **4 GiB**（2 GiB/試行 の見積り、上限 128 GiB）。
- `llm_calls_made 0`、`assumptions` に係数（chars_per_token 4 等）を明記。
読み方の注意: 係数は仮置きで RunMetrics 由来の実測ではない。見積りは「予算内で実行可能か」の事前判定であり、実性能・実費用ではない。実測 token が無い値は export で observed/partial/missing を区別し、欠測を 0 にしない。

## 3. 受け入れ条件ごとの証拠
- 条件: campaign.md に dry-run・見積り・限界の節 — 本ファイルの §2・§5（`grep` で `dry-run` `見積り` `限界` を確認）。
- 条件: crate の試験が通る — `cargo test -p celeris-campaign`: exit 0、**40 件全通過**（lib 11 / cli_dryrun 2 / pilot_fixtures 8 / routing_eval_identity 3 / runner_checker 16、failed 0）。
- 条件: dry-run が mock で通る — `dry-run --manifest crates/campaign/fixtures/pilot.json`: exit 0、total_trials 12、within_budget true。
- 条件: run→check→export が通り、Markdown だけで合格しない — 上記 §1 全て exit 0。`check-report` は報告の `result` 行を生記録の再計算値と照合し、誤り入りレビュー fixture 6 件（passed 水増し・分母欠落・split 混在・失敗記録の削除等）を `pilot_fixtures` 試験で全て検出、対照（誤り無し）は合格。
- 条件: 再起動・依存待ち・障害・予算上限の制御 — `runner_checker`（再開、隔離、`stop_on_budget` で cancelled が分母に残る、改ざん 10 種）と `pilot_fixtures`（制御台本で fail/timeout/error/欠測が分母に残る）。
- 条件: routing-eval 再利用箇所の集計識別/split 回帰 — `routing_eval_identity` 3 件通過。
- 条件: workspace 試験・clippy — 前回 attempt の独立判定で `bash scripts/dev/test-parallel.sh` exit 0（passed 5188, failed 0）、`cargo clippy --workspace -- -D warnings` exit 0。本 WU はドキュメントのみの追加でコード無変更。

## 4. 比較計画
- A = 既存の強い運用 baseline（既存 harness 再利用、後付け制限なし）、B = Celeris + fixed、C = Celeris + Jev。
- 主比較は A/B（全 5 問）。B/C は事前登録 subset（impl と review の 2 問）のみ。subset は manifest に宣言済みで run 中に変更不可。
- 判定は各問題の trial_no 最小の記録。failed/error/timeout/cancelled/missing は分母に残す。dev pilot と final は split を分け、hash・split の違う結果は checker が拒否して混ぜない。試行ごとに workspace/KB を snapshot から分離。

## 5. 実装済み / 未検証の区別と限界
実装済み・mock 上で試験済み: manifest 検証と hash/split、dry-run 見積り、runner（隔離・再開・予算停止）、独立 checker、export、check-report、代表 5 段 fixture、誤り入りレビュー fixture。

未検証・未実施（限界）:
- 実 LLM 測定は未実施。mock の pass/fail は seeded で実性能ではない。5 問・1 試行では統計的主張はできない。
- 実 Celeris 実行器は未実装（`Executor` trait の接合点のみ）。実 token/cost の採取も未実装。
- 見積り係数は仮置きで未較正。runner は逐次で `max_parallel` は見積りのみ。
- summary.json と記録を両方改ざんされると hash/split/版/隔離以外では捕まらない（外部署名なし）。
- fixture の受入 command（`cargo test` 等）は mock では実行しない。題材は Benchfs/Pluvio から縮めた写しで実物ではない。

## 6. 提案
- `Executor` の Celeris 実装（task 作成経路＋RunMetrics から tokens/cost 採取）を次段で作る。
- 実測前に人の予算決定（measure-budget）を取り、まず pilot を少数で実測して見積り係数を較正する。
- trials.jsonl の hash を run 終了時に別媒体へ固定する。
