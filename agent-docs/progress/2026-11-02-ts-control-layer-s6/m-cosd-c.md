---
title: S6 m-cosd-c — 旧 Rust 経路 cos-digest の warm 試行（補い 6 試行）と 20 試行の連結
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-cosd-c — 旧 Rust 経路 cos-digest の warm 試行（補い 6 試行）と 20 試行の連結

unit m-cosd-c。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない。既存の [cos-digest-warm-a.jsonl](cos-digest-warm-a.jsonl)・[cos-digest-warm-b.jsonl](cos-digest-warm-b.jsonl) は書き換えていない（`git diff --quiet HEAD --` で差分 0）。

## 結論
- 旧 Rust 経路の cos-digest warm は 20 試行に達した（a: rep 1〜7、b: rep 8〜14、c: rep 15〜20）。rep に穴・重複は無い。
- 補いの 6 試行（rep 15〜20）は全段 exit 0、編集はすべて戻った（`reverted: true`）。
- 連結 [cos-digest-warm.jsonl](cos-digest-warm.jsonl) は a 88 行・b 88 行・c 82 行を行を変えずに順に連結した 258 行（prime 18・step 120・estimate 120）。`--validate` は problems 0。
- 20 試行の p50 344.491 秒、p95 346.576 秒（最近傍順位）。cos-ts warm（p50 7.470 秒、p95 7.604 秒）に対する短縮率は p50 97.83%、p95 97.81%。
- test の合否: rep 1〜20 の 20 対で、旧 Rust 経路の test-run と cos-ts の trial はともに exit 0。一致は 20/20。

## cos-ts と並べた p50・p95（最近傍順位、秒）
| 経路 | scenario | 試行 | p50 | p95 | 最小 | 最大 |
|---|---|---|---|---|---|---|
| 旧 Rust | cos-digest warm（[cos-digest-warm.jsonl](cos-digest-warm.jsonl)） | 20 | 344.491 | 346.576 | 342.847 | 346.660 |
| TS | cos-ts warm（[cos-ts-warm.jsonl](cos-ts-warm.jsonl) の trial 行） | 20 | 7.470 | 7.604 | — | — |

- 短縮率（1 − TS/Rust）: p50 97.83%、p95 97.81%。p50 の比は約 46 倍。
- 検証の範囲は同じでない: Rust 経路は `-p task-core -p task-dispatch` の check・build・clippy・全試験と release build を含む（[cos-ts-scenario](cos-ts-scenario.md)）。
- rep 15〜20 の試行の合計（step の secs の和、`trial_secs` と一致）: 343.559・344.608・343.224・342.847・346.660・344.599。6 試行とも `over_300s: true`、busy 0、他の cargo・rustc 0。
- 試行中の load average（1 分）は 9.98〜15.0。他の cargo・rustc は 0 なので自分の build の負荷である。rep 1〜14 の範囲（343.005〜346.576）と同じ幅に収まる。

## 識別の照合
| series | host | clean | tree（task-core / task-dispatch） | rustc | commit |
|---|---|---|---|---|---|
| m-cosd-a | home-dev | true | b85f3c34… / efe35b61… | 1.98.1 (48a229cea) | 前半の head |
| m-cosd-b | home-dev | true | b85f3c34… / efe35b61… | 1.98.1 (48a229cea) | 後半の head |
| m-cosd-c | home-dev | true | b85f3c34… / efe35b61… | 1.98.1 (48a229cea) | 「integrate wu/overhead (phase s6-pilot)」 |

- 全 prime・step 行で host・clean・tree hash・rustc が 3 系列とも同じ。違うのは `series` と `commit` で、commit の間の差は測る crate の外（tree hash が同じ）。p50・p95 に prime・estimate は入れていない。

## 実行したこと（直列に 1 回）
1. 開始前（15:40 UTC）: load average 0.90, 2.57, 4.62、cargo 0・rustc 0、`/local` の空き 129 GB（300 GB 中 169 GB 使用）、`git status --porcelain` 空
2. `timeout -s TERM 3050 bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 6 --rep-start 15 --series m-cosd-c --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-c.jsonl` → exit 0（15:40:29〜16:21:56 UTC、timeout に掛かっていない）
3. `cat cos-digest-warm-a.jsonl cos-digest-warm-b.jsonl cos-digest-warm-c.jsonl > cos-digest-warm.jsonl`

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/edit-activate-baseline.sh --validate …/cos-digest-warm-c.jsonl` → exit 0（records 82、prime 6・step 36・estimate 40、problems 0）
- `bash scripts/dev/edit-activate-baseline.sh --validate …/cos-digest-warm.jsonl` → exit 0（records 258、problems 0）
- `bash scripts/dev/edit-activate-baseline.sh --check-targets …/cos-digest-warm.jsonl` → exit 0（cos-digest warm: trials 20、over_300s 20、busy 0、p50 344.491、p95 346.576、problems 0）
- コードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11）

## 未解決
- なし（20 試行の不足は解消した）。

## 提案
- なし。
