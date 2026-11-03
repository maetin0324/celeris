---
title: S6 m-policy-a — 旧 Rust 経路 policy の warm を 10 試行（前半）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 m-policy-a — 旧 Rust 経路 policy の warm を 10 試行（前半）

完了日: 2026-11-03（unit m-policy-a）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従った。コードと script は変えていない。IEEE Access・K2・K4 の release・state dir・config には触れていない。

## 結論
- policy の warm は 10 試行すべて exit 0 で、編集もすべて戻った（`reverted: true`）。合計の中央値は 234.943 秒で、S0 の 221.015 秒より 13.928 秒（6.3%）長い。
- この差を Rust 経路の遅れと断定はできない。S0 と今回は commit が違い、測った対象のコードが増えている（下の「S0 との違い」）。TS 経路との比較には、S0 でなく今回と後半（m-policy-b）の 20 試行を使う。
- p50・p95 の確定は 20 試行が揃う s6-report で行う。この file の値は前半 10 試行だけの集計である。
- 生データ: [policy-warm-a.jsonl](policy-warm-a.jsonl)（prime 6 行＋step 60 行＋estimate 40 行）。付記 S5-10 の表の file 名（`policy-a.jsonl`）でなく、unit の objective の名前で置いた。部分 file はこの名前のまま残し、連結 file だけを raw-names で `policy.jsonl` に改名した（[raw-names](raw-names.md)）。

## S0 との比較（warm、秒）
中央値どうしを並べた。S0 は 3 試行、今回は 10 試行で、試行数が違う。

| 段 | S0 中央値 | 今回 中央値 | 差 | 今回の最小〜最大 |
|---|---|---|---|---|
| check | 16.814 | 17.872 | +1.058 | 17.588〜18.130 |
| build | 52.581 | 56.088 | +3.507 | 55.842〜56.572 |
| clippy | 18.596 | 19.691 | +1.095 | 19.486〜20.003 |
| test-build | 32.341 | 35.148 | +2.807 | 34.709〜35.768 |
| test-run | 13.004 | 13.674 | +0.670 | 12.685〜14.305 |
| release-build | 87.673 | 92.796 | +5.123 | 91.714〜93.577 |
| 試行の合計 | 221.015 | 234.943 | +13.928 | 233.488〜236.162 |

- 試行の合計は、S0 が 219.657・221.015・224.836、今回が 234.748・234.432・234.009・234.719・236.162・236.058・235.792・235.966・235.138・233.488（rep 1〜10 の順）。範囲は重ならない。
- 段の中央値の和は 235.269 秒で、試行の合計の中央値とは一致しない（中央値は段ごとに別の試行から取るため）。
- 最近傍順位では、今回の 10 試行の p50 が 234.748 秒、p95 が 236.162 秒（n=10 なので p95 は最大値）。上の表の 234.943 秒は 5 番目と 6 番目の平均である。
- prime（編集なしの 1 周。集計に入れない）は 303.6 秒、S0 は 289 秒。

## S0 との違い
| 項目 | S0 | 今回 |
|---|---|---|
| host | 行に host の欄が無い。CPU は同じ型番 | home-dev |
| CPU | AMD Ryzen Threadripper PRO 3945WX、論理 24 | 同じ |
| commit の件名 | `phase 1: question で終わった run の usage を runs と worker_finished に記録する` | `phase s6: m-ts-cold: 記録から hash に見える値を除く` |
| rustc・cargo | 1.98.1・1.98.1 | 同じ |
| cargo 設定 | `CARGO_INCREMENTAL=0`、`line-tables-only` | 同じ |
| sccache | server に届かない（`reachable: false`） | 同じ |
| target | S0 の WU の scratch。prime で温めた | この WU の scratch。prime で温めた（開始時 688 MB） |
| 時刻（UTC） | 2026-11-02 05:56〜06:07 に warm の step を開始 | 2026-11-03 11:44〜12:23 |
| 段の行の `env.loadavg` の 1 分値 | 3.98〜13.44 | 3.43〜14.52 |

S0 の commit から今回の HEAD までのコードの差（`git diff --stat`）:
- `crates/task-core`: 36 file、+7605 / -57
- `crates/celeris`: 29 file、+8679 / -38。`crates/celeris/Cargo.toml` に `celeris-script`（deno_core を使う script runtime）の依存が増えた
- `Cargo.lock`: +985 / -22（増えた package 93 個）

この増分が差の原因かは切り分けていない（未確認）。増えた分が大きい段が再 compile の多い build・test-build・release-build であることとは矛盾しない。

## 開始前の状態（2026-11-03 11:39 UTC、host home-dev）
- `git status --porcelain` → 空
- `uptime` → `load average: 0.59, 1.82, 4.30`
- 他の cargo・rustc: `pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0
- `/local` の空き 131 GB（測定中の記録で 128.8 GB）

## 実行したこと
1. `bash scripts/dev/edit-activate-baseline.sh --scenario policy --mode warm --reps 10 --series m-policy-a --out agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-warm-a.jsonl` → exit 0（11:39:17〜12:23:39 UTC）。同じ run の中で cargo・試験を並べていない
   - `--series` は objective のコマンドに無いが、付記 S5-10 が系列名に leaf の key を求めるので付けた。`--rep-start` は既定の 1
2. `--validate` を実行し、この file を書いて commit

## 識別と負荷
- 全 66 行（prime・step）で `env.identity` は同じ値: `clean: true`、`series: m-policy-a`、`target_kind: warm-shared`、host home-dev、rustc 1.98.1、node v24.14.1、pnpm 12.6.0、`commit` と `tree.crates/task-core` も 1 種類。値は jsonl にある。m-policy-b と連結するときは tree hash を照合する（commit はこの file の commit の分だけ変わる）
- `over_300s` は 0 件、busy は 0 件（全試行で試行の前後と最中の他の cargo・rustc が 0）
- 試行の前後の 1 分 load average は 9.70〜14.52。測定自身の cargo が含まれる。cargo・rustc 以外の process の寄与は分けていない（未確認）
- estimate の 40 行は release 記録の読み取りで、測定値ではない。集計に入れていない

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/policy-warm-a.jsonl` → exit 0（records 106、prime 6・step 60・estimate 40、problems 0）
- `sh scripts/dev/progress-index.sh --check` → exit 0
- 測定 leaf はコードを変えないので test-affected・clippy・fmt は走らせていない（付記 S5-11 の 3・5）

## 未解決
- 無し（p50・p95 の確定と a・b の連結は s6-report の担当）。

## 提案
- s6-report は、S0 の 221 秒を「移行前の値」として TS 経路と比べない。今回の a・b の 20 試行を旧 Rust 経路の値にし、S0 との 6.3% の差は S1〜S5 でコードと依存が増えた後の値として別に記す。
