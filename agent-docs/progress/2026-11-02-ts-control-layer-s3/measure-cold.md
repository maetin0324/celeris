---
title: S3 measure-cold — policy-ts の cold 20 試行と目標の照合
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 measure-cold — policy-ts の cold 20 試行と目標の照合

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit measure-cold。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
cold は目標を満たした。20 試行の p95 は 11.525 秒（目標 120 秒以下）、p50 は 5.570 秒。
全試行で exit 0、分類 `ts-only`、10 段すべて exit 0、cargo の起動 0 回、`link_count` 0、編集を戻せた。
p95 は baseline の policy cold（283.5 秒）の 4.1%、cos cold（377.2 秒）の 3.1% である。
ただし、ここでの cold は `control-ts/node_modules` を消した状態で、runner の cache は当たる。baseline の cold は使い捨ての cargo target から build する。条件が違うので、下の「比べるときの注意」と合わせて読む。

生データ: [policy-ts-cold.jsonl](policy-ts-cold.jsonl)（21 行。prime 1 行、trial 20 行）。

## 全体
最近傍順位（n=20 なら p50 は 10 番目、p95 は 19 番目）。`secs` は編集を書いた時点から段 activate が終わるまで。編集を戻す時間と、`node_modules` を消す時間は入らない。

| 指標 | policy-ts cold | 目標 | baseline policy cold（1 試行） | 比 | baseline cos cold（1 試行） | 比 |
|---|---|---|---|---|---|---|
| p95 | 11.525 秒 | 120 秒以下 | 283.5 秒 | 4.1% | 377.2 秒 | 3.1% |
| p50 | 5.570 秒 | なし | 283.5 秒 | 2.0% | 377.2 秒 | 1.5% |
| 最小・最大 | 5.099 秒・14.352 秒 | なし | なし | なし | なし | なし |
| cargo の起動 | 全試行 0 回 | 0 回 | 6 段すべて cargo | なし | 6 段すべて cargo | なし |
| `link_count` | 全試行 0 | 0 | 103 回（段の和） | なし | 215 回（段の和） | なし |

## 段ごとの秒
policy-ts の p50・p95・最大は 20 試行から、baseline は S0 の `baseline/policy.jsonl`・`baseline/cos.jsonl` の cold 1 試行から出した。
対応づけは measure-warm と同じこの task の判断で、同じ処理ではない。比は policy-ts の p50 と baseline policy cold の比である。

| policy-ts の段 | p50 | p95 | 最大 | 対応する baseline の段 | policy cold の秒 | cos cold の秒 | 比（policy） |
|---|---|---|---|---|---|---|---|
| classify | 0.036 | 0.065 | 0.065 | なし | なし | なし | なし |
| runner-cache | 0.173 | 0.292 | 0.627 | なし | なし | なし | なし |
| install | 0.220 | 0.779 | 1.100 | なし | なし | なし | なし |
| typecheck | 0.087 | 0.225 | 0.338 | check | 22.9 | 37.6 | 0.4% |
| 対応なし（TS の経路に lint の段は無い） | なし | なし | なし | clippy | 18.7 | 34.0 | なし |
| bundle | 0.080 | 0.188 | 0.258 | build（celeris debug） | 61.9 | 61.8 | 0.1% |
| unit + golden + safety + integration | 4.688（p50 の和） | なし | なし | test-build + test-run | 48.7（35.5 + 13.2） | 112.9（76.8 + 36.1） | 9.6% |
| 　unit | 0.880 | 2.456 | 2.921 | | | | |
| 　golden | 0.307 | 0.609 | 0.776 | | | | |
| 　safety | 0.040 | 0.077 | 0.100 | | | | |
| 　integration | 3.461 | 7.125 | 8.748 | | | | |
| activate | 0.076 | 0.150 | 0.398 | release-build | 131.3 | 131.0 | 0.1% |
| 全体 | 5.570 | 11.525 | 14.352 | 合計 | 283.5 | 377.2 | 2.0% |

段の外の時間: 全試行の `secs` から 10 段の秒の和を引くと、中央値 0.304 秒（0.253〜1.334 秒）。内訳は測っていない。
最も長い段は integration で、p50 全体の約 6 割を占める。warm（S3 measure-warm）との差は p50 で 0.54 秒（5.031 → 5.570 秒）で、そのうち install が 0.218 秒（0.002 → 0.220 秒）である。

## 比べるときの注意
- 冷やした範囲が違う。policy-ts の cold は `control-ts/node_modules` を消すだけで、runner の cache は当たる（prime の `script-runner-cache.sh ensure` は 0.179 秒）。pnpm の store も在る状態で、install は `--offline` である。baseline の cold は cargo target を作り直す。
- runner の cache が外れた場合はこの 20 試行に入っていない。wire の記録では `ensure` が cache の無い状態から 60.6 秒かかる。この 60.6 秒に今回の最大 14.352 秒を足すと約 75 秒で 120 秒以下だが、これは足し算の推定で実測ではない。
- 単体試験の範囲は広い。`node_modules` が無いと import グラフを計算できないので、20 試行すべて全部の単体試験を実行した（`units` が `ALL`）。
- cos の比は参考である。cos の baseline は Rust の `job_wait_extend.rs` への数十行の編集で、policy-ts の編集（`lane_rules.ts` の規則 1 件）とは編集の中身が違う。cos の TS の scenario は測っていない。
- 反映はどちらも本番では測っていない。policy-ts の activate は runner が一時の state dir に行う。baseline の release-build は binary を作るだけで、promote は入らない。
- policy-ts の秒に、Rust の全試験・release gate は入らない。これらは release gate で走る（ADR 2026-11-02-ts-fast-verify）。
- baseline は 1 試行、policy-ts は 20 試行で、測った時期も host の負荷も違う。

## 環境
| 項目 | 値 |
|---|---|
| HEAD の件名 | `phase s3: measure-warm: policy-ts の warm 20 試行を測り目標と照合する` |
| 編集 | `control-ts/src/domains/lane_rules.ts` の `RULES` に当たらない規則を 1 件（nonce は 20 試行とも別） |
| CPU | AMD Ryzen Threadripper PRO 3945WX 12-Cores、論理 24 |
| 負荷 | 試行の終了時の loadavg(1 分) 6.57〜26.12。CPU を焼く負荷はかけていない |
| 他の計測 | 開始の前に `edit-activate-baseline` の process が無いことを確かめた。measure-warm は完了済み |
| 他の cargo/rustc | 試行の終了時に 0〜24 個（他の task と推定。この計測は cargo を起動しない）。開始時には他の task の `cargo nextest` が動いていた |
| 実行時間 | 20 試行全体で 2 分 28 秒 |

## 外れ値
rep 1〜9 と rep 13・14 は 5.099〜5.592 秒だった。rep 10〜12・15〜20 は 6.964〜14.352 秒で、最大は rep 12（14.352 秒）である。
rep 10 から loadavg(1 分) が 13 以上に上がったので、host の他の処理と重なったと見ている。ただし rep 13・14 は loadavg 22 以上でも 5 秒台で、原因は未確認である（切り分けていない）。p95 の 11.525 秒は遅かった試行の 1 つ（rep 10）の値で、この試行では integration が 8.748 秒だった。

## 変更
- `phase s3: measure-cold: policy-ts の cold 20 試行を測り目標と照合する` — 生データとこの記録を足した。script と数値は直していない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 20 試行の記録 | `bash scripts/dev/edit-activate-baseline.sh --scenario policy-ts --mode cold --reps 20 --out agent-docs/progress/2026-11-02-ts-control-layer-s3/policy-ts-cold.jsonl` | exit 0、2 分 28 秒 |
| 欄の検査 | `bash scripts/dev/edit-activate-baseline.sh --validate <file>` | exit 0、`records` 21、`problems` 0 |
| 目標の照合 | `bash scripts/dev/edit-activate-baseline.sh --check-targets <file>` | exit 0、`trials` 20、`p95` 11.525、`p95_limit` 120.0、`problems` 0 |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0、`試験対象の変更なし`（文書検査は実行された） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| 進捗の索引 | `sh scripts/dev/progress-index.sh --check` | exit 0 |

## 未解決
- runner の cache が外れた状態の cold は測っていない（「比べるときの注意」の約 75 秒は推定）。
- 遅い試行の原因は未確認である。

## 提案
なし。
