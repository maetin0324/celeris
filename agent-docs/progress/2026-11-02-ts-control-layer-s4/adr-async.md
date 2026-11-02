---
title: S4 adr-async — 境界 ADR に digest pass 全体の async 経路（付記 S4-11〜S4-19）
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 adr-async — 境界 ADR に digest pass 全体の async 経路

完了日: 2026-11-02（unit adr-async）。コードは変えていない。

## 結論
[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) に「付記 2026-11-02: S4 の digest pass 全体を TS の async 経路にする」（S4-11〜S4-19）を足した。
前回の final review の不合格理由は 3 点だった。TS に移ったのは同期の `decide()` だけだった。async の読み出しと apply は Rust に残っていた。TS の command はどの mode でも apply されなかった。
この付記で、TS の `cosDigestPass(ctx)` が op（`cos_digest:list_runs`・`cos_digest:read_run`）を await して pass 全体を動かすことにした。`cos_digest` に限って、mode `ts` で TS の command を検証→apply する。

## 目的の項目と節の対応
| 目的の項目 | 節 | 要点 |
|---|---|---|
| 改める点の明示（S4-4・S4-6・S4-7 ほか） | S4-11 | S4-1〜S4-9 のうち改める点を表にした |
| (1) pass 全体 | S4-12 | op 2 つ、出力 `CosDigestPassOutput`、pass の memo（1 pass で store を読むのは list 1 回、run ごとに 1 回）、bounded mpsc の host sink、deadline 200 ms。`digest_due`・`dirty`・起点・retry は Rust |
| (2) capability・scope・context 分離 | S4-13 | capability 2 つ、grant は domain に結び付ける、`read_run` は list の答えの (thread, run_id) だけ。V8 context は分けない。代わりに capability 要求の排他・domain 結び付け・token 失効・正逆 2 回の replay で塞ぐ |
| (3) mode | S4-14 | `compare` は凍結した memo から op に答える。`ts` は規則 7〜9 を足して検証し、失敗・timeout・検証落ち・`mode_changed` で pass 全体を Rust で apply する。`→ts` は人の actor だけで、元は同じ generation の `compare` に限る。本番は `rust` のまま |
| (4) 記録 | S4-15 | `script_compare_log` の json に `mode`・`pass_id`・`applied_by`・`fallback`・`ops` を足す。冪等 key は不変。新しい event の variant は無い |
| (5) trace | S4-16 | `pass_traces.jsonl`（1 行 1 pass、`ops` の要求/応答と `output`）。fake host で replay する。`cases.jsonl` は 1 run の pass に組み直して replay する |
| (6) 旧経路の削除 | S4-17 | `PolicyScript::cos_digest`、run ごとの compare、`cosDigest`、`traces.jsonl`、`cos_digest_trace.rs` |
| (7) await 中の error | S4-18 | deadline は `timeout`、cancel は `terminated`、作り直しは `generation_gone`、mode 切替は `mode_changed` の fallback |
| 後続 unit の範囲と試験名 | S4-19 | host-ops・mode-ts・pass-wire・ts-pass・traces-async・real-host・robust-ts・close-async |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ control-ts/` → exit 0（差分なし）
- `grep -c '^### S4-1[1-9] ' agent-docs/adr/2026-11-02-ts-control-layer-boundary.md` → 9
- `bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 結果は run の報告に書く

## 未解決
1. 要求ごとに isolate を作り直す方式が pass の deadline 200 ms に収まるかは未確認（S4-13）。op を使う領域が 2 つ以上になったら見直す。
2. mode `ts` の dispatcher は、1 tick で TS を最大 200 ms 待つ。tick の遅れの許容は S5 の停止基準で決める。
3. host-ops と pass-wire の間で、`ScriptCall` の host sink の型をどちらの unit が決めるか。S4-19 では host-ops が `celeris-script` 側の汎用の sink を作り、pass-wire が task-dispatch の trait と celeris の実装で使う、としている。

## 提案
- S5 で mode `ts` の canary を決めるときに、`script_compare_log` の `applied_by`・`fallback` を停止基準の分母に使う。
