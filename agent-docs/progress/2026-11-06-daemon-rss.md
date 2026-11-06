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

## 再現の途中結果（Run #2、before、dev build、条件 off、240 秒）
条件: 本番 backup `20261106-073408-pre-9ed88afa4506.sqlite3`（1.4 GB）の写しを scratch（`/local/celeris/data/scratch/daemon-rss/`、repo 外）に置き、`daemon_soak.py run --cond off`（fake harness、127.0.0.1 のみ、task 30 秒に 1 件、標本 15 秒）で起動。生の標本は `2026-11-06-daemon-rss/before/samples.jsonl`・`ops.jsonl`（DB の写しは含まない）。
- 起動直後 rss_anon 15 MB → 3.6 秒後 72 MB → 33 秒後 120 MB（malloc_arena は 7 MB のまま、増えた分は main arena 外）。
- 63 秒後に malloc_arena 117 MB（in_use 24 / free 92）、123 秒後に 128 MB。以降 243 秒まで rss_anon は 120→133 MB（約 +13 MB / 4 分）、free は 90〜119 MB で in_use は 10〜37 MB。
- 解釈: 本番規模の DB で、起動 1 分以内に「使用中 24 MB に対し空き 92 MB」の arena が現れた。本番と同じ形（使用中が小さく空きが大きい）だが、規模は小さい（本番は 324 MB に跳ね、930 MB まで増加）。240 秒では本番の傾きは再現していない。差は実負荷（本番は threads が 44 まで増える。この run は 25〜29）と推測するが未確認。
- 確保元（どの tick・API・処理か）は未特定。

## 未実施（次の run）
1. （部分的に済み。上記）より長い run、API 負荷（一覧・inbox）を足した run で傾きを再現する。
2. tick 別・API 別に `malloc_free` の増分を見て確保元を特定する。
3. 修正後、同じ条件で before/after を測り、`rss_anon` の傾き・`malloc_free`・tick の時間・dispatch の遅延を本節に書く。
4. 受け入れ条件 0〜2（test-affected・clippy・fmt）はコード変更後に実行する。今回はコード変更なし。

## 本番への移行
未定（コードの `mallopt` で足りれば不要。環境変数が必要なら再現後にここへ書く）。

## 提案
ADR の方針のとおり、まず大きな一時確保の削減（A）を狙う。
