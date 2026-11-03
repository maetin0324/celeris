---
title: S6 m-cosd-b — 旧 Rust 経路 cos-digest の warm 試行（後半）と連結
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-cosd-b — 旧 Rust 経路 cos-digest の warm 試行（後半）と連結

unit m-cosd-b。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない。

## 結論
- 試行数は未達（連結で 14/20）。後半は 7 試行（rep 8〜14）で、全段 exit 0、編集もすべて戻った。1 試行が約 345 秒かかり、prime と合わせて run の壁時計 3600 秒に 7 試行しか入らない（[m-cosd-a](m-cosd-a.md) と同じ理由）。足りない 6 試行を推定で埋めない。
- rep は m-cosd-a の提案に従い 8 から数えた（`--rep-start 8`）。rep に穴は無い。
- 連結 [cos-digest-warm.jsonl](cos-digest-warm.jsonl) は前半 88 行・後半 88 行を行を変えずに順に連結した 176 行（prime 12・step 84・estimate 80）で、`--validate` は problems 0。objective の「20 行」は試行数の意味に取ったが、試行は 14 である。
- test の合否: rep 1〜14 で旧 Rust 経路の test-run は 14/14 exit 0、cos-ts の同じ rep の trial も 14/14 exit 0。一致は 14/14（cos-ts の rep 15〜20 も exit 0 だが対になる Rust の試行が無い）。

## cos-ts と並べた p50・p95（最近傍順位、秒）
| 経路 | scenario | 試行 | p50 | p95 | 最小 | 最大 |
|---|---|---|---|---|---|---|
| 旧 Rust | cos-digest warm | 14 | 344.491 | 346.576 | 343.005 | 346.576 |
| TS | cos-ts warm（[cos-ts-warm.jsonl](cos-ts-warm.jsonl) の trial 行） | 20 | 7.470 | 7.604 | — | — |

- 短縮率（1 − TS/Rust）: p50 97.83%、p95 97.81%。p50 の比は約 46 倍。
- 検証の範囲は同じでない: Rust 経路は `-p task-core -p task-dispatch` の全試験と release build を含む（[cos-ts-scenario](cos-ts-scenario.md)）。
- Rust の試行の合計（rep 1〜14）: 343.005・344.788・344.824・343.619・345.115・344.341・344.839・346.576・345.653・344.208・344.031・344.510・343.940・344.491。14 試行とも `over_300s: true`、busy 0。

## 識別の照合
- 前後半とも全 prime・step 行で `clean: true`、host home-dev、tree hash（`crates/task-core` b85f3c34…、`crates/task-dispatch` efe35b61…）が同じ。違うのは `series`（m-cosd-a / m-cosd-b）と `commit`（間の差は進捗と jsonl だけ）。p50・p95 に prime・estimate は入れていない。

## 実行したこと（直列）
1. 開始前（14:01 UTC）: load average 4.44, 7.26, 7.55、cargo・rustc 0、`/local` の空き 129 GB、`git status --porcelain` 空
2. `timeout -s TERM 3050 bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 7 --rep-start 8 --series m-cosd-b --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-b.jsonl` → exit 0（14:01:20〜14:48:33 UTC）
3. `cat cos-digest-warm-a.jsonl cos-digest-warm-b.jsonl > cos-digest-warm.jsonl`

## 証拠（提出時の HEAD で実行）
- `--validate cos-digest-warm-b.jsonl` → exit 0（records 88、problems 0）
- `--validate cos-digest-warm.jsonl` → exit 0（records 176、problems 0）
- `--check-targets cos-digest-warm.jsonl` → trials 14、over_300s 14、busy 0、p50 344.491、p95 346.576、problems 0
- コードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11）

## 未解決
- cos-digest の warm は 14 試行で、受け入れ条件の 20 試行に 6 足りない。

## 提案
- 計画に cos-digest の測定 leaf を 1 本足す（`--reps 6 --rep-start 15 --series m-cosd-c`、出力 cos-digest-warm-c.jsonl を連結）。1 run に 7 試行が上限なので 6 試行は収まる。
