---
title: "campaign 代表 fixture・pilot manifest・誤り入りレビュー fixture・手順"
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# campaign-fixtures（WU fixtures）

## 実装済み
- `crates/campaign/fixtures/`
  - `problems.json`（source）→ `input-manifest.json`（routing-eval manifest・dev split・hash 固定）。代表 5 problem: 実装（Benchfs chunk index）/ 短い実験（Benchfs 小実験）/ 分析（Benchfs scaling）/ 報告（Pluvio survey 要約）/ レビュー（数値整合）。題材は既存の Benchfs/Pluvio 案件から縮めたもの（実 Benchfs の写しではない）。
  - `repo/` `materials/` `kb/`: 初期 snapshot の mock（materials は pilot.json に sha256 を固定）。
  - `pilot.json`: dev split・A/B/C・版（A=claude-code baseline / B=celeris+fixed / C=celeris+jev-1）・予算上限（1M token / 36000 s / 128 GiB / 並列 2）・終了条件・B/C 事前 subset（impl と review の 2 問）。
  - `control/mock-script.json`: 制御用 mock 台本（fail/timeout/error/欠測を決まった試行に置く）。
  - `review/cases.json` + `review/report-good.md`: 誤り入りレビュー fixture の定義。記録側（passed 旗の食い違い＝数値不一致、失敗記録の削除＝分母欠落、split 混在）と報告側（passed 水増し、分母 n 欠落、split 混在）の 6 件。
- `src/report.rs` と CLI `check-report --manifest --out --report`: 報告の `result <cfg> passed=<x> n=<y> split=<s>` 行を、checker が生記録から再計算した値と照合（Markdown だけでは合格しない）。
- 試験 `crates/campaign/tests/pilot_fixtures.rs` 8 件: fixture が現行生成結果と一致、5 段を含み dev のみ、pilot dry-run exit 0 と見積り、mock pilot の run→check→export→check-report（CLI）、制御台本で失敗・timeout・error・欠測が分母に残る、予算停止で cancelled が分母に残る、誤り 6 件すべてを checker が検出（誤り無しの対照は合格）、悪い報告で CLI が非 0。
- `docs/architecture-map.md` に campaign の 1 行を追加。

## 実行手順
```
cargo run -p celeris-campaign -- dry-run --manifest crates/campaign/fixtures/pilot.json   # exit 0・見積り JSON（estimated）
cargo run -p celeris-campaign -- run --executor mock --manifest crates/campaign/fixtures/pilot.json --out $TMPDIR/out \
  --repo-src crates/campaign/fixtures/repo --materials-src crates/campaign/fixtures/materials --kb-src crates/campaign/fixtures/kb
cargo run -p celeris-campaign -- check --manifest crates/campaign/fixtures/pilot.json --out $TMPDIR/out
cargo run -p celeris-campaign -- check-report --manifest crates/campaign/fixtures/pilot.json --out $TMPDIR/out --report crates/campaign/fixtures/review/report-good.md
cargo run -p celeris-campaign -- export --manifest crates/campaign/fixtures/pilot.json --out $TMPDIR/out
```
fixture の作り直し: `CAMPAIGN_UPDATE_FIXTURES=1 cargo test -p celeris-campaign --test pilot_fixtures`（problems.json・materials を変えたとき。直後に通常実行で一致を確認）。

## 比較計画
- 主比較 A/B: 全 5 問。A=既存の強い運用 baseline（既存 harness 再利用・後付け制限なし）、B=Celeris+fixed。
- 副比較 B/C: 事前登録 subset 2 問のみ（C=Celeris+Jev）。subset は manifest に宣言済みで run 中に変えられない。
- 判定は各問題の trial_no 最小の記録。failed/error/timeout/cancelled/missing は分母に残す。dev と final、hash の違う manifest の結果は混ぜない（checker が拒否）。

## 限界
- mock 実行器の結果（seeded の pass/fail 混合）は実性能ではなく、pilot の数値は基盤の動作確認だけ。5 問・1 試行では統計的主張はできない。
- fixture の problem は縮めた題材で、受入 command（`cargo test` 等）は mock 実行器では実行しない。
- dry-run の係数は仮置き（`assumptions`）。mock は逐次実行で `max_parallel` は見積りのみ。

## 実装済み / 未検証の区別
- 実装済み・試験済み: 上記すべて（mock 実行器上）。
- 未検証・未実施: 実 LLM 測定、Celeris 経路の実行器（`Executor` trait の実装）、実 token/cost の採取、見積り係数の実測較正。実 LLM 測定は人の予算決定（measure-budget）後。有料測定はしていない。

## 証拠
- `cargo test -p celeris-campaign`: 全通過（pilot_fixtures 8 件を含む）。`celeris-campaign dry-run --manifest crates/campaign/fixtures/pilot.json`: exit 0、total_trials 12、within_budget true。
- workspace 試験・clippy は完了報告に記載。

## 提案
- 次段で `Executor` の Celeris 実装（task 作成経路＋RunMetrics）を作り、measure-budget 決定後に pilot を実測へ。
