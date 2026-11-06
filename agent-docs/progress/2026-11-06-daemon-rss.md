---
title: 本番 daemon の RSS 増加（glibc malloc arena の空き保持）の原因特定と修正
tasks: [01M6QZ031RC9YK3E6T12ESCR80]
status: running
updated: 2026-11-06
---
# 本番 daemon の RSS 増加

状態: 途中。ADR と本番 raw の取り込みまで完了。再現・修正・before/after は未実施（run の予算 600 秒・10 turn を使い切るため）。

## 済んだこと
- ADR `agent-docs/adr/2026-11-06-daemon-rss-malloc-arena.md`（切り分けの方法、候補 A〜F の比較、採否の基準）を実装より先に commit。
- 本番 raw `2026-11-06-daemon-rss/prod-process-memory-9ed88afa-20261106.jsonl`（44 行）を取り込み。

## 未実施（次の run）
1. 本番 DB backup の写しを scratch に置き、fake harness のみ・外部接続なしの config で `scripts/dev/daemon-soak/` を動かして再現する（写しは repo・KB・progress に入れない）。
2. tick 別・API 別に `malloc_free` の増分を見て確保元を特定する。
3. 修正後、同じ条件で before/after を測り、`rss_anon` の傾き・`malloc_free`・tick の時間・dispatch の遅延を本節に書く。
4. 受け入れ条件 0〜2（test-affected・clippy・fmt）はコード変更後に実行する。今回はコード変更なし。

## 本番への移行
未定（コードの `mallopt` で足りれば不要。環境変数が必要なら再現後にここへ書く）。

## 提案
ADR の方針のとおり、まず大きな一時確保の削減（A）を狙う。
