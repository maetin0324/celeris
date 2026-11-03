---
title: S5 — generation 管理・段階切替・rollback（rust→compare→canary→ts）と停止基準の監視
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: running
updated: 2026-11-03
---
# S5 — generation 管理・段階切替・rollback

task 01M6FPPJN7A8H427MXFVYFY7H4（S5→S6。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）。S5 の段の記録。S6 の生データは `agent-docs/progress/2026-11-02-ts-control-layer-s6/` に置く。

## 現在地
- 設計: [境界 ADR](../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-1〜S5-12 を adr-s5 unit が書いた（[adr-s5](2026-11-02-ts-control-layer-s5/adr-s5.md)）。コードは未変更。
- 後続の unit の作業範囲と試験名は付記 S5-12 の表のとおり。実装は未着手。

## 検査（adr-s5、提出時の HEAD）
adr-s5 unit の結果は [adr-s5](2026-11-02-ts-control-layer-s5/adr-s5.md) と下に書く。

## 未解決
1. S4 未解決 7（tick の遅れ）と 8（bundle の無い環境の SKIP）は付記 S5-8・S5-9 で方式を決めた。実装と試験は canary-dispatch・bundle-skip unit が行う（未実装）。
2. S5-5 のとおり、S5 の 3 領域には長期 workflow が無いので、checkpoint の表は作らない。照合の関数と拒否の経路だけを作る。最終 review で「長期 workflow の引き継ぎ」を実物で求められた場合、確かめられるのは作り物の版の集合による照合の試験だけである。
3. S6 の測定の `cos.jsonl` は、S0 の `cos`（`job_wait_extend`）でなく新しい `cos-digest` の 2 つの部分を連結した file になる（付記 S5-10）。名前が S0 の scenario と同じなので、報告で中身の `scenario` を明記する。

## 提案
1. release gate（`scripts/selfdeploy/release.sh`）でも付記 S5-9 と同じく `$TMPDIR` に bundle を作り `CELERIS_SCRIPT_BUNDLE_DIR` を渡す。今は release gate の全体試験で bundle を使う試験が SKIP しうる（S4 未解決 8 と同じ）。release gate の変更は別 task にする。
2. S4 の提案 2（message と requeue を 1 transaction で書く store の API）は付記 S5-6 の「副作用の後の照合」と関わるが、挙動の変更なので S5 に混ぜない。別 task のまま。

## 本番への移行
S5 の実装が入った release を本番に出すとき、人が行う（worker は行わない）:
1. 本番の config（`~/.config/celeris/` の下）に `[script] active_bundle_sha256` があれば消す（付記 S5-1。新しい binary は読み込みエラーにする）。`[script]` が無ければ何もしない。確認: 新しい binary で `celeris` を起動し、config の読み込みエラーが出ないこと。
2. 本番の mode は全領域 `rust` のまま。`compare`・`canary`・`ts` にするのは人の判断で、手順は s5-close unit が書く。
