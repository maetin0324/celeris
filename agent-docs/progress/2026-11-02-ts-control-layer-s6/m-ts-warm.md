---
title: S6 m-ts-warm — policy-ts・cos-ts の warm を各 20 試行
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-ts-warm — policy-ts・cos-ts の warm を各 20 試行

完了日: 2026-11-03（unit m-ts-warm）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない（測定は `$TMPDIR` の一時 state dir だけを使う）。

## 結論
- policy-ts・cos-ts の warm は、どちらも 20 試行すべて exit 0 で、目標（p50 30 秒・p95 60 秒）に合格した。
  | scenario | 試行 | p50（秒） | p95（秒） | 最大（秒） | over_300s | busy |
  |---|---|---|---|---|---|---|
  | policy-ts warm | 20 | 7.519 | 7.694 | 7.722 | 0 | 0 |
  | cos-ts warm | 20 | 7.470 | 7.604 | 7.608 | 0 | 0 |
- 生データ: [policy-ts.jsonl](policy-ts.jsonl)・[cos-ts.jsonl](cos-ts.jsonl)（各 prime 2 行＋trial 20 行）。測定時は unit の objective の名前（`-warm` 付き）で置き、2026-11-03 raw-names で S5-10 の表の名前に改名した。対応は [raw-names](raw-names.md)。この file の下のコマンドも改名後の名前で書いた。

## 開始前の状態（2026-11-03 11:26 UTC、host home-dev、24 CPU）
- `uptime` → `up 18 days, 20:39, 3 users, load average: 7.96, 11.27, 8.38`
- 他の cargo・rustc: `pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0
- 測定中の試行前の 1 分 load average: policy-ts 1.87〜5.32、cos-ts 1.50〜1.93。自分以外の cargo・rustc は全試行の前後で 0（busy の印 0 件）

## 実行したこと（直列。測定と並べて cargo・試験は走らせていない）
1. この file に開始前の状態を書いて commit（`git status --porcelain` を空にする）
2. `bash scripts/dev/edit-activate-baseline.sh --scenario policy-ts --mode warm --reps 20 --series m-ts-warm --out agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-ts.jsonl` → exit 0（11:26:55〜11:29:34 UTC）
3. 生データを commit（次の測定の `clean` を保つため）
4. `bash scripts/dev/edit-activate-baseline.sh --scenario cos-ts --mode warm --reps 20 --series m-ts-warm --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-ts.jsonl` → exit 0（11:29:38〜11:32:18 UTC）
5. 生データを commit し、`--validate` と `--check-targets` を両 file に実行

## 識別（env.identity）
- 両 file とも全試行で `clean: true`、`reverted: true`、`tree.control-ts` は同じ値、`target_kind: none`、`series: m-ts-warm`、host `home-dev`、rustc 1.98.1、node v24.14.1、pnpm 12.6.0、`host_api` 1.1.0
- `commit` は policy-ts と cos-ts で違う（間に生データの commit が入った。control-ts の tree hash は同じ）。`bundle_sha256` は試行ごとに違う（編集の nonce が試行ごとに違うため）

## 最遅 3 試行の内訳（秒。test = unit＋golden＋safety＋integration）
| scenario | rep | 合計 | typecheck | bundle | test（unit / golden / safety / integration） | activate |
|---|---|---|---|---|---|---|
| policy-ts | 12 | 7.722 | 0.094 | 0.077 | 6.929（1.660 / 0.809 / 0.039 / 4.421） | 0.071 |
| policy-ts | 7 | 7.694 | 0.096 | 0.075 | 6.912（1.624 / 0.793 / 0.039 / 4.456） | 0.066 |
| policy-ts | 15 | 7.683 | 0.102 | 0.076 | 6.886（1.647 / 0.811 / 0.043 / 4.385） | 0.063 |
| cos-ts | 9 | 7.608 | 0.093 | 0.082 | 6.811（1.608 / 0.798 / 0.040 / 4.365） | 0.066 |
| cos-ts | 7 | 7.604 | 0.097 | 0.078 | 6.809（1.620 / 0.807 / 0.043 / 4.337） | 0.070 |
| cos-ts | 10 | 7.602 | 0.096 | 0.075 | 6.801（1.628 / 0.801 / 0.039 / 4.333） | 0.070 |

残りは classify（約 0.11 秒）・runner-cache（約 0.18 秒）・install（0.002 秒）。時間の大半は integration（約 4.4 秒）と unit（約 1.6 秒）で、typecheck は 0.1 秒以下だった。

## 証拠（提出時の HEAD で実行）
- `--validate policy-ts.jsonl` → exit 0（records 22、prime 2・trial 20、problems 0）
- `--validate cos-ts.jsonl` → exit 0（records 22、prime 2・trial 20、problems 0）
- `--check-targets policy-ts.jsonl` → exit 0（trials 20、p50 7.519 / 30.0、p95 7.694 / 60.0、problems 0）
- `--check-targets cos-ts.jsonl` → exit 0（trials 20、p50 7.47 / 30.0、p95 7.604 / 60.0、problems 0）
- 測定 leaf はコードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11 の 3・5）

## 未解決
- typecheck（`control-ts/node_modules/.bin/tsc --noEmit`）が 0.1 秒以下と短い。どの TypeScript の実装・cache が効いているかは未確認。s6-report で「TS の typecheck 秒」を書くときに確かめる。

## 提案
- file 名の違いは raw-names で S5-10 の表の名前に揃えて解いた（[raw-names](raw-names.md)）。
