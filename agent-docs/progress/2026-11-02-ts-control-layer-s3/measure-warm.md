---
title: S3 measure-warm — policy-ts の warm 20 試行と目標の照合
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 measure-warm — policy-ts の warm 20 試行と目標の照合

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit measure-warm。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
warm は目標を満たした。20 試行の p50 は 5.031 秒（目標 30 秒以下）、p95 は 7.406 秒（目標 60 秒以下）。
全試行で exit 0、分類 `ts-only`、10 段すべて exit 0、cargo の起動 0 回、`link_count` 0、編集を戻せた。
p50 は baseline（S0 の policy warm 中央値 221.0 秒）の 2.3%（約 1/44）である。
ただし 2 つの経路は検証の中身が違う（下の「比べるときの注意」）。この数値は、1 件の編集で Rust の release build まで待つ時間と、TS の検証を通す時間の比である。

生データ: [policy-ts-warm.jsonl](policy-ts-warm.jsonl)（22 行。prime 2 行、trial 20 行）。

## 全体
最近傍順位（n=20 なら p50 は 10 番目、p95 は 19 番目）。`secs` は編集を書いた時点から段 activate が終わるまで。編集を戻す時間は入らない。

| 指標 | policy-ts warm | 目標 | baseline（policy warm） | baseline との比 |
|---|---|---|---|---|
| p50 | 5.031 秒 | 30 秒以下 | 221.0 秒（3 試行の中央値） | 2.3% |
| p95 | 7.406 秒 | 60 秒以下 | なし（3 試行では出せない。最大 224.8 秒） | 中央値 221.0 秒に対して 3.4% |
| 最小・最大 | 4.645 秒・7.807 秒 | なし | 219.7 秒・224.8 秒 | なし |
| cargo の起動 | 全試行 0 回 | 0 回 | 6 段すべて cargo | なし |
| `link_count` | 全試行 0 | 0 | 試行あたり 3 回（build・test-build・release-build） | なし |

## 段ごとの秒
policy-ts の p50・p95・最大は 20 試行から、baseline は S0 の `baseline/policy.jsonl` の warm 3 試行の中央値から出した。
対応づけはこの unit の判断で、同じ処理ではない。比は p50 と baseline の中央値の比である。

| policy-ts の段 | p50 | p95 | 最大 | 対応する baseline の段 | baseline の秒 | 比 |
|---|---|---|---|---|---|---|
| classify | 0.105 | 0.121 | 0.122 | なし | なし | なし |
| runner-cache | 0.169 | 0.182 | 1.003 | なし | なし | なし |
| install | 0.002 | 0.002 | 0.002 | なし | なし | なし |
| typecheck | 0.082 | 0.088 | 0.090 | check | 16.8 | 0.5% |
| 対応なし（TS の経路に lint の段は無い） | なし | なし | なし | clippy | 18.6 | なし |
| bundle | 0.072 | 0.362 | 0.973 | build（celeris debug） | 52.6 | 0.1% |
| unit + golden + safety + integration | 4.227（p50 の和） | なし | なし | test-build + test-run | 45.3（32.3 + 13.0） | 9.3% |
| 　unit | 0.813 | 1.321 | 2.398 | | | |
| 　golden | 0.300 | 0.319 | 0.341 | | | |
| 　safety | 0.038 | 0.042 | 0.043 | | | |
| 　integration | 3.076 | 3.336 | 3.368 | | | |
| activate | 0.072 | 0.217 | 0.276 | release-build | 87.7 | 0.1% |
| 全体 | 5.031 | 7.406 | 7.807 | 合計 | 221.0 | 2.3% |

段の外の時間: 全試行の `secs` から 10 段の秒の和を引くと、中央値 0.26 秒（0.248〜0.893 秒）。編集の書き込みと script の処理の分と見ている（内訳は測っていない）。
最も長い段は integration で、p50 全体の約 6 割を占める。

## 比べるときの注意
- 反映はどちらも本番では測っていない。policy-ts の activate は runner が一時の state dir に行う。baseline の release-build は binary を作るだけで、promote は入らない。
- policy-ts の 5 秒に、Rust の全試験・release gate は入らない。これらは release gate で走る（ADR 2026-11-02-ts-fast-verify）。baseline の task-core の試験とは試験の範囲も違う。
- runner の build の秒は入っていない。prime の `script-runner-cache.sh ensure` は cache が当たって 0.17 秒だった。
  cache が無いときの `ensure` は wire の記録で 60.6 秒である。host の source など cache key の入力が変わると、この時間が 1 回かかる。
- baseline は 3 試行、policy-ts は 20 試行で、測った時期も host の負荷も違う。

## 環境
| 項目 | 値 |
|---|---|
| HEAD の件名 | `integrate wu/bench (phase bench)` |
| 編集 | `control-ts/src/domains/lane_rules.ts` の `RULES` に当たらない規則を 1 件（nonce は 20 試行とも別） |
| CPU | AMD Ryzen Threadripper PRO 3945WX 12-Cores、論理 24 |
| 負荷 | 試行中の loadavg(1 分) 最大 8.27〜12.63。CPU を焼く負荷はかけていない |
| 他の計測 | measure-cold は同時に走っていない（計測の前後で process と作業場所が無いことを確かめた） |
| 他の cargo/rustc | 試行の終了時に 0〜5 個（他の task と推定。この計測は cargo を起動しない）。試行中の標本では 0。標本は 1 試行 1〜2 点で少ない |

## 外れ値
遅かったのは次の 2 試行で、他の 18 試行は 4.645〜5.524 秒だった。原因は未確認である（host の他の処理か、ファイルキャッシュかを切り分けていない）。
- rep 2（7.406 秒）: unit 2.398 秒。
- rep 3（7.807 秒）: runner-cache 1.003 秒、bundle 0.973 秒、unit 1.321 秒。

## 変更
- `phase s3: measure-warm: policy-ts の warm 20 試行を測り目標と照合する` — 生データとこの記録を足した。script と数値は直していない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 20 試行の記録 | `bash scripts/dev/edit-activate-baseline.sh --scenario policy-ts --mode warm --reps 20 --out agent-docs/progress/2026-11-02-ts-control-layer-s3/policy-ts-warm.jsonl` | exit 0、1 分 55 秒 |
| 欄の検査 | `bash scripts/dev/edit-activate-baseline.sh --validate <file>` | exit 0、`records` 22、`problems` 0 |
| 目標の照合 | `bash scripts/dev/edit-activate-baseline.sh --check-targets <file>` | exit 0、`trials` 20、`p50` 5.031、`p95` 7.406、`problems` 0 |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0、`試験対象の変更なし`（文書だけの変更。文書検査は実行された） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| 進捗の索引 | `sh scripts/dev/progress-index.sh --check` | exit 0 |

## 未解決
- cold の計測は unit measure-cold が行う。

## 提案
なし。
