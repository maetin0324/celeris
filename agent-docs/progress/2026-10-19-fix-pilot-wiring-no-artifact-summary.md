---
tasks: [01M58VFR9ZS5RG7QNB558DT4XK]
status: done
completed: 2026-10-19
title: pilot_wiring no_artifact 試験が gate で落ちる原因と修正（summary の no_artifact を記録から数える）
updated: 2026-10-19
---
# summary.json の budget.no_artifact が 0 になる原因と修正

## 原因
`RunReport.no_artifact` は `run()` 1 回ごとの加算（旧: 0 起点）で、`summary.json` は `run()` のたびに全体を書き直す。
async executor では A の試行を回収した run と、`waiting == 0` を返す最後の run が別になり得る（負荷時に B/C の投入や回収が次の pass にずれる）。
後ろの run は何も回収しないので no_artifact = 0 で summary.json を上書きし、trials.jsonl には reason=no_artifact が残るのに summary が 0 になる（`s["budget"]["no_artifact"]` が 0、期待 1）。
他の指標（tokens_missing, executor_errors など）は既に記録から数え直していて、no_artifact だけが取り残されていた。単独実行で通るのはタイミングが揃うため。

## 修正
`runner.rs`: `RunReport` の初期化で、既存記録のうち reason == REASON_NO_ARTIFACT の件数を起点にする（再開をまたぐ累計）。今回の run の加算はそのまま。

## 証拠
- `cargo test -p celeris-campaign`: 全 binary ok（failed 0）
- 全体は `bash scripts/dev/test-parallel.sh` / clippy / fmt を下記で実行。
