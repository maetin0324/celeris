---
title: CoS chat prompt cache baseline（T2）
tasks: [01M4D3XKDZEKDCBR0ZE78KD078]
status: done
updated: 2026-10-08
---

# CoS chat prompt cache baseline（T2）

## 実装

- `crates/task-worker/src/cos_chat/bench_tests.rs`: 決定的 prompt-bytes ベンチ（LLM 無し）。`cargo test -p task-worker cos_chat_bench_ -- --nocapture` が JSON を出し、`COS_CHAT_BENCH_OUT` で保存する。新規 thread 10 件の先頭一致、固定部/可変部、同 thread 10 turn の stdin bytes、`HEADLESS_RUN_NOTE`・skill の bytes を測る。
- `scripts/dev/cos-chat-bench.sh`（+ `.py`、台本 `cos-chat-bench-script.json`）: `cos-chat-live.sh` と同じ隔離方式（試験用 data dir・一時 DB・port 17942）の live ベンチ。台本は S1 新規 10・S2 同 thread 10 turn（相談/起票/コメント）・S5 warm 3（30 秒）/cold 3（370 秒）。chat run の telemetry（T1 の API）と stream-json の 5m/1h 内訳・model を集め、runs.csv/json・summary.json・tables.md を出す。`enrich` mode で LLM を呼ばずに既存 evidence を再集計できる。
- 既存の worker 挙動は変えていない（試験とスクリプトの追加のみ）。

## 結果

baseline 本体は task の artifacts（`cos-chat-bench-baseline.{json,md}`）。要点: 新規 thread の先頭一致 43 B（固定 4,226 B）、新規 thread の cache write 21.9k・cache read 83.5k・非 cache input 8、approx_tokens と context 長のずれ約 21 倍、cache は 1h TTL で 370 秒の cold に差が無い。claude-code 26 run・codex 5 run（全て completed）。

## 未解決事項

subscription 枠の消費、codex の cache write/TTL/model/skill 読込、acp/pi（未実施）、account_id、skill 配送時間（H5）、61 分超の cold は「不明」。

## 提案

- H1 の判定基準は「非 cache input」でなく cache write と cache read で測り直す（非 cache input は既に 8 token）。
- T3 で `approx_tokens` を最新 turn の context 長にする案（H4）を採る見込み。
