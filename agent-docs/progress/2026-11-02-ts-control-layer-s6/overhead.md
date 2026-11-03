---
title: S6 overhead — 判定遅延の内訳・RSS・isolate 数・型検査の秒・cold build の増分
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 overhead — 許容 overhead の測定

unit overhead。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の (f) の許容 overhead と付記 S5-8・S5-11 に従った。数値の全体は [overhead.json](overhead.json) にある。

## 結論
今回測った (f) の許容 overhead 7 項目は、すべて閾値内だった。未測定は `task-dispatch`・`task-api` の test-build の増分（閾値 2 秒以下）の 1 項目で、判定していない。

| 項目（(f)） | 測定値 | 閾値 | 判定 |
|---|---|---|---|
| 判定 1 回の遅延 p50（warm） | 0.087 ms | ≤ 1 ms | 内 |
| 判定 1 回の遅延 p99（warm） | 0.129 ms | ≤ 5 ms | 内 |
| daemon の RSS 増分 generation 1 個（isolate 1 個） | +23.2 MiB | ≤ 100 MiB | 内 |
| daemon の RSS 増分 generation 2 個（active と drain 中、isolate 2 個） | +27.3 MiB | ≤ 200 MiB | 内 |
| `celeris` の cold release build の増分 | +9.8 秒（131.3 → 141.1） | ≤ 60 秒（191 秒以下） | 内 |
| warm の `cargo build -p celeris` の link の増分（m-policy の生データ） | +0.44 秒（1.36 → 1.795） | ≤ 5 秒 | 内 |
| `celeris` の release binary の増分 | +67.0 MiB（79.6 → 146.6） | ≤ 80 MiB | 内（余裕 13.0 MiB） |
| `task-dispatch`・`task-api` の test-build の増分 | 未測定 | ≤ 2 秒 | 未判定 |

付記 S5-8 の tick の遅れも許容内だった。p95 4.0 ms（許容 200 ms）、最大 5.6 ms、`tick_budget` 0 件。

S1 と比べると、cold build は 134.6 → 141.1 秒、binary は 143.7 → 146.6 MiB に増えた。binary の余裕は S1 の 15.8 MiB から 13.0 MiB に減った。cold build は各 1 試行なので、数秒の差は誤差の範囲にある。

## 判定 1 回の遅延の内訳（ms、p50 / p99。各 1000 回×3 run の最大）
| 領域 | 全体 | queue | 境界（入） | JS 実行 | 境界（出） | 応答の起床 |
|---|---|---|---|---|---|---|
| `host_selftest`（op 1 回） | 0.093 / 0.136 | 0.017 / 0.029 | 0.008 / 0.014 | 0.037 / 0.066 | 0.006 / 0.011 | 0.019 / 0.028 |
| `lane_rules`（純粋判定） | 0.064 / 0.094 | 0.017 / 0.028 | 0.008 / 0.013 | 0.013 / 0.033 | 0.003 / 0.005 | 0.018 / 0.027 |
| `route_priority`（純粋判定） | 0.086 / 0.233 | 0.017 / 0.027 | 0.018 / 0.030 | 0.020 / 0.155 | 0.005 / 0.007 | 0.018 / 0.027 |
| `cos_digest` の pass（op 2 回） | 0.203 / 0.378 | 0.017 / 0.117 | 0.006 / 0.009 | 0.137 / 0.192 | 0.014 / 0.025 | 0.018 / 0.029 |
| `cos_digest` の pass（op 21 回） | 1.731 / 2.246 | 0.017 / 0.075 | 0.009 / 0.020 | 1.476 / 1.928 | 0.197 / 0.264 | 0.019 / 0.029 |

- 区間の定義: queue = 送信から actor の取り出しまで（queue の深さは 0 で、thread の起床を含む）。境界（入）= Grant の用意と入力の V8 変換。JS 実行 = script の入口から future の完了まで（event loop と op の受け渡しを含む）。境界（出）= 出力の JSON 変換と stats。応答の起床 = oneshot の送信から呼び手の起床まで。
- `cos_digest` の op の答えは記録（`pass_traces.jsonl`）を測定用の op server が返した。store は読んでいない。実運用の pass の遅延は store の読み出しの分だけ長くなる。
- `cos_digest` の pass の p99 は 2.246 ms で、(f) の deadline 200 ms の 1.1% に当たる。`lane_rules` の p99 は 0.094 ms で、deadline 50 ms の 0.2% に当たる。

## RSS と isolate 数（非本番 daemon、MiB、3 回）
baseline は `[script]` が無い daemon で、45.94・46.62・46.45 MiB だった。

| 状態 | isolate | 増分（rep 1 / 2 / 3） |
|---|---|---|
| `[script]` あり、generation なし | 0 | 1.05 / 0.37 / 0.35 |
| generation 1 個（active） | 1 | 23.18 / 23.06 / 22.48 |
| generation 2 個（active と loaded） | 2 | 27.11 / 27.18 / 26.04 |
| generation 2 個（active と draining） | 2 | 27.30 / 27.18 / 26.04 |
| 旧 generation を退役した後 | 1 | 27.03 / 26.75 / 25.57 |

- draining は切替の直後（0.003〜0.004 秒後）に読み、3 回とも API が `draining`・thread が 2 本だった。in-flight は 0 で、要求を抱えた drain の RSS は測っていない。
- 退役で isolate は 1 個に減るが、RSS は 0.3〜0.5 MiB しか減らなかった。切替を繰り返したときに RSS が増え続けるかは未確認。
- daemon の isolate は呼び出しを受けていない。参考の script-bench（warm up と 1000 回の呼び出しの後）は isolate 1 個 +24.2 MiB、2 個 +31.3 MiB だった。

## 型検査と bundle の秒（生データから。各 20 試行）
| 系列 | typecheck p50 / p95 | bundle p50 / p95 |
|---|---|---|
| `cos-ts` warm | 0.095 / 0.097 | 0.075 / 0.079 |
| `policy-ts` warm | 0.096 / 0.103 | 0.075 / 0.079 |
| `cos-ts` cold | 0.095 / 0.099 | 0.075 / 0.079 |

## tick の遅れ（付記 S5-8）
非本番 daemon（`tick_ms` 2000）で task 20 件を fake harness で流し、130 秒測った。`lane_rules`・`route_priority` は canary 9999 bps にした（API の上限。計数は一時 DB に synthetic に積んだ）。

- 59 tick の集計: p50 0 µs、p95 4033 µs、最大 5566 µs、`tick_budget` 0 件。`script_compare_recorded` 40 件。終了時の mode は両領域とも canary のままで、自動の戻しは起きなかった。
- p95 の 4 ms は、判定 1 回の 0.06〜0.09 ms より大きい。同期の tick から別の runtime で待つ分の費用と推測するが、内訳は未確認。
- 負荷の小さい測定で、1 tick に多数の判定が重なる場合は測っていない。

## 測り方
本体のコードと script は変えていない。測定は提出時の HEAD を `git archive` で `$TMPDIR` に展開した写しで行った。

1. cold build: 渡された `CARGO_TARGET_DIR` の下の新しい subdir `overhead-cold-head` で `SCCACHE_BIN=/nonexistent cargo build --release --locked -p celeris --bin celeris` を 1 回走らせ、`date +%s%N` の差で 141.116 秒だった。開始前の 1 分 load average は 1.73（自分の build の後は 13.08）、他の cargo・rustc は 0。S0 の 131.3 秒は別の run の 1 試行で、同じ host・rustc 1.98.1・sccache 無しの値。
2. 遅延の (f) の判定と参考の RSS: 変更なしの `script-bench --bundle-dir control-ts/dist --iters 1000` を 3 回走らせ、各欄の最大を取った（S1 と同じ）。
3. 遅延の内訳: script-bench は内訳を出さないので、写しに [overhead-probe.patch](overhead-probe.patch) を当てて測った。時刻の記録と測定用の binary を足しただけで、repo には入れていない。計器ありの全体は `host_selftest` p50 0.093 ms で、計器なしの 0.087 ms より 0.006 ms 大きい。
4. RSS・isolate 数: [overhead-daemon.py](overhead-daemon.py) の `rss` で測った。daemon は変更なしの release binary で、一時 DB・一時 state dir・loopback の port を使い、終わったら止めて消した。generation 2 個目は pilot の [candidate.patch](pilot/candidate.patch) を当てた bundle を使った。isolate 数は `/proc/<pid>/task/*/comm` が `celeris-script` の thread の数で数えた。
5. tick の遅れ: daemon の `script_tick_wait` の log 行は無い（S5 未解決 4）。写しの `tick_loop.rs` に 30 tick ごとの出力を足し（同じ patch）、[overhead-daemon.py](overhead-daemon.py) の `tick` で測った。
6. 負荷をかける script は使っていない。本番の mode・deploy・config・DB には触れていない。IEEE Access・K2・K4 の環境にも触れていない。LLM は呼んでいない。

環境: home-dev、AMD Ryzen Threadripper PRO 3945WX（論理 24）、kernel 6.8.12-9-pve、rustc 1.98.1、node v24.14.1、`CARGO_INCREMENTAL=0`。

## 証拠（提出時の HEAD で実行）
- overhead.json の必須欄 8 個の有無を python で検査: 欠け 0。
- `bash scripts/dev/test-affected.sh`: exit 0（試験対象の変更なし。進捗の file だけ）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: ok。`sh scripts/dev/check-doc-links.sh`: ok。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は `agent-docs/progress/2026-11-02-ts-control-layer-s6/` の overhead の 4 file だけを出す。

## 未解決事項
1. `task-dispatch`・`task-api` の test-build の増分は未測定（(f) の閾値 2 秒以下）。この 2 crate は S1 の時点で `deno_core` を依存に持たない。
2. 要求を抱えた drain の RSS は未測定。
3. 退役を繰り返したときの RSS の推移は未確認。
4. tick の遅れの p95 4 ms の内訳は未確認。
5. `script_tick_wait` の log 行は本体に無い（S5 未解決 4 のまま）。この測定は写しに足して読んだ。

## 提案
- `tick_loop.rs` に `script_tick_wait` の log 行を足す（S5 の提案 3 と同じ）。本番の canary で tick の遅れを読めるようにするため。
- release binary の余裕は 13.0 MiB まで減った。V8 の feature や TS 側の依存を増やす変更では、この測定を再実行する。
