---
title: S6 m-policy-b — 旧 Rust 経路 policy の warm を 10 試行（後半）と 20 試行の連結
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-policy-b — 旧 Rust 経路 policy の warm を 10 試行（後半）と 20 試行の連結

完了日: 2026-11-03（unit m-policy-b）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない。

## 結論
- 後半 10 試行（rep 11〜20）はすべて exit 0 で、編集もすべて戻った（`reverted: true`）。`over_300s` 0 件、busy 0 件。
- 前半 [policy-warm-a.jsonl](policy-warm-a.jsonl) と後半 [policy-warm-b.jsonl](policy-warm-b.jsonl) を行を変えずに順に連結し、[policy-warm.jsonl](policy-warm.jsonl) を作った。`--validate` は exit 0（records 212: prime 12・step 120・estimate 80、problems 0）。
- objective の「20 行」は 20 試行の意味に取った。policy の 1 試行は step 6 行で記録されるので、20 試行の連結は 212 行になる（付記 S5-10「行を変えずに a・b を順に連結」に従った）。
- 前後半の中央値の差は -0.23%（20% 以内）なので、理由の候補は書かない。

## 20 試行の p50・p95（最近傍順位、秒）
旧 Rust 経路 policy は試行の合計（6 段の和）、TS 経路 policy-ts は [policy-ts-warm.jsonl](policy-ts-warm.jsonl) の trial 行の `secs`（m-ts-warm の値）。

| 経路 | scenario | 試行 | p50 | p95 | 最小 | 最大 |
|---|---|---|---|---|---|---|
| 旧 Rust | policy warm | 20 | 234.481 | 236.162 | 233.488 | 236.383 |
| TS | policy-ts warm | 20 | 7.519 | 7.694 | — | 7.722 |

- p50 の比は約 31 倍（234.481 / 7.519）。検証の範囲は同じでない: Rust 経路は task-core の全試験と release build を含み、TS 経路より広い（付記 S5-10）。
- 段ごとの 20 試行の p50 / p95: check 17.906 / 18.106、build 56.075 / 56.495、clippy 19.647 / 19.895、test-build 35.092 / 35.768、test-run 13.523 / 13.852、release-build 92.505 / 93.577。

## 前半と後半（試行の合計、秒）
| 系列 | rep | p50 | p95 | 中央値 | 最小〜最大 |
|---|---|---|---|---|---|
| m-policy-a | 1〜10 | 234.748 | 236.162 | 234.943 | 233.488〜236.162 |
| m-policy-b | 11〜20 | 234.394 | 236.383 | 234.403 | 233.928〜236.383 |

後半の各試行の合計: 233.928・234.394・234.767・236.383・234.411・234.009・235.491・234.317・234.481・234.351（rep 11〜20 の順）。

## 識別の照合（連結の前提）
- a・b とも全行で `clean: true`、`tree.crates/task-core` が同じ値、host home-dev、rustc 1.98.1、node v24.14.1、pnpm 12.6.0、`target_kind: warm-shared`。
- 違うのは `commit`（a は m-ts-cold の最後の commit、b は m-policy-a の最後の commit。間の差は進捗の file と jsonl だけ）と `series`（m-policy-a / m-policy-b）。どちらも付記 S5-10 の「同じ集計に入れない」条件（tree hash・toolchain・host）には当たらない。
- p50・p95 には prime と estimate の行を入れていない（付記 S5-11 の 7）。

## 開始前の状態と負荷（host home-dev）
- 2026-11-03 12:25:54 UTC: `git status --porcelain` → 空、`load average: 2.78, 6.64, 6.96`、`pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0、`/local` の空き 129 GB
- 段の行の `env.loadavg` の 1 分値は 4.13〜14.25。測定自身の cargo が含まれる。それ以外の process の寄与は分けていない（未確認）

## 実行したこと（直列。測定と並べて cargo・試験は走らせていない）
1. `bash scripts/dev/edit-activate-baseline.sh --scenario policy --mode warm --reps 10 --rep-start 11 --series m-policy-b --out agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-warm-b.jsonl` → exit 0（12:25:54〜13:10:13 UTC）
   - `--rep-start 11`・`--series` は付記 S5-10 に従って付けた
2. `cat policy-warm-a.jsonl policy-warm-b.jsonl > policy-warm.jsonl`
3. `--validate` を 2 file に実行し、この file を書いて commit

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-warm-b.jsonl` → exit 0（records 106、problems 0）
- `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-warm.jsonl` → exit 0（records 212、problems 0）
- `sh scripts/dev/progress-index.sh --check` → 下の commit 後に実行
- 測定 leaf はコードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11 の 5）

## 未解決
- 無し。

## 提案
- s6-report は旧 Rust 経路 policy の値に policy-warm.jsonl の 20 試行（p50 234.481、p95 236.162）を使う。付記 S5-10 の表の file 名 `policy.jsonl` でなく、この名前で置いた。
