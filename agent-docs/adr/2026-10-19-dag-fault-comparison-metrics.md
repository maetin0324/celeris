# dagbench 指標の再生成（生ログ形式・出力 table・掃引）

- 日付: 2026-10-19
- 状態: 採用（実装済み: `crates/campaign/src/dagbench/metrics.rs`, `run.rs`）
- tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
- 親: `2026-10-19-dag-fault-comparison-driver.md` の D3/D5/D6 を実装する WorkUnit `metrics` の判断

## 決定
- M1. raw dir の形式: `run.json`（入力の写し: DAG・点・枠・seed。結果を含めない）、`ext.jsonl`（ext-sim ledger）、
  `worker.jsonl`（開始・終了・消費の独立記録）、`control.jsonl`（再起動・再開・手動介入）、`samples.jsonl`（`/proc` 標本）。
  指標はこれらだけから計算し、系の state file や集計値は読まない。出力に時刻・乱数を入れず、再生成は byte 同一。
- M2. baseline に `worker.jsonl` の書出し（`set_worker_log`）と node の `release_at`（campaign 到着時刻）を足した。
  baseline は N campaign を 1 つの DAG（node id `c{i}/<node>`）として流すので、Celeris と同じ `concurrent_campaigns` を持てる。
- M3. 時間は整数の論理秒、百分位は最近順位法。浮動小数を使わない（再生成の同一性のため）。
- M4. 制御 CPU/RSS は `/proc/<pid>/stat`・`status` の読み取りのみ（開始・終了の標本）。baseline・Celeris とも driver を
  in-process で動かすので `control_scope` に「driver process 全体（ext-sim の書込みを含む上限値）」と明記する。
  Celeris daemon だけの CPU は、子 process 化した driver（faults WU/accept WU）で pid を分けて標本化する。
- M5. 出力は `control_measured_metrics.json`（schema `celeris.dagbench-metrics/1`）と 1 行 TSV。成功率・費用・token の欄は持たず、
  `TrialRecord` / `final_accounting`（`llm_campaign`）と別 file・別 schema。`condition`（`normal` / `fault:<id>`）は条件のラベルで、
  研究問題の数として数えない（D7）。dataset が `control_measured` でない raw は読まない。
- M6. 固定点の掃引は `metrics::sweep_items`（scenario × 7 点 × seed × 系の一覧を返すだけ。実行は呼び出し側）。本 unit では実行しない。
- M7. `dagbench run --arm celeris` は CLI からは実行しない（task-api の router は dev-dependency）。`CelerisSide::drive` と
  `RunHeader::from_spec` を使う駆動は試験/accept WU が行う。

## 却下した案
- baseline の state file から ready 待ちを読む: 系の自己申告になる。worker 記録と scenario の依存辺から独立に計算する。
- task-api を本依存にして CLI から Celeris arm を動かす: 本番 crate の依存を測定のために広げる。accept WU で必要なら再検討。
