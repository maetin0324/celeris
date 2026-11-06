---
title: tick_cos_chat_launch の行数比例の調査と修正
tasks: [01M6QZ6KYWW18GYMZYM17SYSW9]
status: running
updated: 2026-11-06
---
# tick_cos_chat_launch の行数比例の調査と修正

## 結論
`claim_queued` と `control_hook` が毎 tick、`chat_thread_list`（`THREAD_SELECT`）で全 thread を走査していた。
`THREAD_SELECT` は thread ごとに 5 個の相関 subquery（実行中 run・queued 件数・未読件数＝`chat_messages` の `seq>read_through_seq`・`cos_inbox_items` 件数）を持つ。
root task ごとに summary thread が増えるので、tick の費用が thread 数と message 数に比例する。これが第一の原因と判断した。
設計は ADR `agent-docs/adr/2026-11-06-cos-chat-launch-scaling.md`（実装より先に commit 済み）。

## 変更（commit は件名）
- n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測（`cos.*` の段別 `step_us`）
- n2: queued 入力と実行中 run の thread だけを引く（全 thread 走査の削除・migration 0119）
  - `chat_thread_ids_with_queued_or_open_run()` を追加。部分 index `idx_chat_messages_queued`・`idx_chat_runs_open` だけを引く。
  - `claim_queued`・`control_hook` はその id だけを `chat_thread_get` で読む。旧い全 thread 走査は両方から削除した。
- 回帰試験 `thread_ids_with_queued_or_open_run_uses_partial_indexes_and_ignores_idle_rows`（task-core）。
  idle thread 300 件＋queued 1 件で、結果が queued の thread を含み、`EXPLAIN QUERY PLAN` が 2 つの部分 index を使うことを固定する。時計・負荷は使わない。

## 未測定・未解決（正直に書く）
- 受け入れ条件 3・5 の「行数と所要時間の対応」「before/after の段の所要時間・dispatch 遅延」は、予算（10 turn・600 秒）内で測れていない。数値は未取得。
  次の手: `TICK_SCALE_*` 形の試験に chat thread の複写（N=100/1000/5000）を足し、修正前後の commit で `cos.claim_queued` の `step_us` を比べる。
- `triage_ingest` の digest_pass・triage_fallback の全件走査、`root_done_advance_all` も行数比例の疑いが残る。段別測定で確定してから直す。
- 本番の傾きが伸びないことの確認は release 後の soak 数値が要る（worker は release・promote をしない）。
- RSS 増加 task 01M6QZ031RC9YK3E6T12ESCR80 とは重複して直していない。この変更は DB 読み取りの走査量を減らすだけで、arena の設定には触れていない。

## 本番への移行

## 本番 DB の写しでの段別計測（operator 依頼 10:20Z）— 結論: 修正は本番の遅さを再現も解消もしていない
手順: `crates/celeris/tests/tick_prod_copy_steps.rs`（`TICK_PROD_DB` に backup `20261106-093409-pre-c9bf4bf7dc76.sqlite3` を渡す手動の計測、既定では走らない）。
一時 dir へ複製した DB を、fake harness だけ・cluster/browser/通知/remote/LLM なしの config で開き、5 tick の `step_us` を出す。本番 DB と state dir には書かない。
修正前は commit「n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測」（段別計測だけ入り、全 thread 走査が残る）、修正後は本 branch の HEAD。単位は µs。tick 0 は起動直後の照合と triage の取り込みを含むので別に書く。

| 段 | 修正前 tick 1〜4 の範囲 | 修正後 tick 1〜4 の範囲 |
|---|---|---|
| tick_cos_chat_launch | 58,837〜76,355 | 150,799〜353,630 |
| cos.claim_queued | 5,115〜6,562 | 2,381〜15,126 |
| cos.control_hook | 6,932〜8,270 | 4,861〜20,336 |
| cos.triage_ingest | 4,537〜5,267 | 9,210〜30,056 |
| cos.relay_rejudge | 41,858〜55,777 | 129,807〜287,493 |

tick 0（修正前 / 修正後）: tick_cos_chat_launch 1,875,090 / 3,498,944、triage_ingest 1,434,820 / 2,823,578。

読み取り:
- 本番規模の DB（chat_messages 10,433・chat_runs 2,931・cos_inbox_items 2,692）でも、修正前の tick_cos_chat_launch は 0.06〜0.08 秒しかかからず、本番の p50 約 20 秒は再現しなかった。claim_queued・control_hook は修正前でも 5〜8 ミリ秒で、全 thread 走査は本番の写しでは遅い段ではない。
- したがって migration 0119 の修正は、この DB の写しでは本番の遅さに効いたと言えない。修正後の値が修正前より大きい理由は未確認（2 回の測定は別の run で、host の負荷・DB 写しのキャッシュ状態の対照を取っていない。修正が tick を遅くする経路は読み取りでは見当たらない）この測定だけでは「悪化していない」とも示せていない。
- 本番だけが遅い原因は未特定。単独 process の写しで再現しない差は、(1) 稼働中の API・web・SSE・notice 同期が同時に SQLite の書き込み lock を持つ競合、(2) 実 daemon が持つ実行中の chat run（`launch.running`）や live_runs、(3) 07:34Z 時点の DB の状態、のいずれかにある可能性がある。いずれも未検証の仮説である。
- 全体の tick（5〜7 秒）で大きいのは recover_reviews（2.6〜3.4 秒）・prune_and_sweep（1.4〜1.9 秒）・dispatch_ready（0.9〜1.6 秒）で、tick_cos_chat_launch ではない。

次の手（提案）: 本番の slow tick log で tick_cos_chat_launch の段別（`cos.*`）が出ているのは c9bf4bf7 以降なので、人が本番の直近の slow tick の `cos.*` の値を共有すれば、どの段が 20 秒かを直接特定できる。写しで再現するには、書き込み lock を持つ別 thread（API 相当）を同時に動かす計測が要る。

## 交互の再測定（修正前 → 修正後 を 3 組、同じ backup の新しい複製から）
同一 host・同一 run の中で、修正前（commit「n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測」）と修正後（main 取り込み・migration 0119 付け替え後の HEAD）の測定用 binary を作り、before / after を交互に 3 組走らせた。tick 0 は除き tick 1〜4。単位は ms。

| 組 | 版 | tick_cos_chat_launch | claim_queued | control_hook | relay_rejudge |
|---|---|---|---|---|---|
| 1 | 修正前 | 57, 57, 57, 57 | 4, 4, 4, 4 | 6, 6, 6, 6 | 41, 41, 41, 41 |
| 1 | 修正後 | 61, 51, 52, 60 | 2, 1, 1, 2 | 3, 3, 3, 4 | 49, 41, 42, 46 |
| 2 | 修正前 | 99, 86, 59, 60 | 8, 7, 5, 5 | 10, 10, 7, 7 | 72, 61, 42, 42 |
| 2 | 修正後 | 51, 52, 51, 56 | 1, 1, 1, 1 | 3, 3, 3, 3 | 41, 42, 42, 46 |
| 3 | 修正前 | 92, 60, 90, 91 | 7, 5, 7, 7 | 11, 7, 10, 10 | 63, 42, 64, 66 |
| 3 | 修正後 | 169, 264, 122, 2909 | 2, 3, 6, 2 | 6, 11, 11, 9 | 108, 142, 93, 71 |

読み取り:
- 組 1 と組 2 の修正後は host が静かで、tick_cos_chat_launch は修正前の 57〜60 ms に対し 51〜56 ms。修正の対象である claim_queued は 4〜5 ms から 1〜2 ms、control_hook は 6〜7 ms から 3 ms に下がった。relay_rejudge は 41〜42 ms で修正前後同じ。修正は relay_rejudge の code に触れていない。
- 組 3 の修正後（tick_cos_chat_launch 122〜2,909 ms、tick 4 は triage_ingest が 2,825 ms）は、全 tick の total も 7〜13 秒に伸びていた。claim_queued は 2〜6 ms と修正前以下なので、修正が遅くした段ではなく、測定中の host の揺れ（原因の process は未確認）と判断する。
- 先の測定（修正前 58〜76 ms、修正後 150〜354 ms）で修正後が大きかったのも同じ揺れと見る。今回の交互測定では「修正後が一貫して遅い」ことは起きなかった。
- 結論: 本番規模の DB の写しでは、修正前でも tick_cos_chat_launch は約 0.06 秒で、本番の 18〜23 秒は再現しない。修正は対象の 2 段を約半分にするが、本番の遅さの原因を取り除いた証拠は得られていない。

## 未解決: 本番の tick_cos_chat_launch が一時的に 20 秒になった原因
operator の journal 集計（celeris@c9bf4bf7dc76、10 分ごとの中央値）: 09:30〜09:40Z は 0.13〜0.16 秒、09:50〜10:10Z は 18〜23 秒（最大 46.6 秒）、10:20Z 以降は 0.11〜0.22 秒に戻り、12:30〜13:10Z は 0.14 秒（最大 0.37 秒）。旧 daemon（9ed88afa）の停止は 10:53:48Z で、遅さが戻ったのはそれより前なので、2 daemon の同居だけが原因とは言えない。
原因は未特定。c9bf4bf7 には cos.* の段別計測が入っていないので、どの段かも分からない。この task の release が入った後の slow tick の cos.* 値で、再発時に段を特定する。

## 検査結果（commit「n2: 修正前後の交互測定の結果と本番の未解決事項を進捗に書く」の時点の HEAD、main 取り込み後）
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 7761 件 passed・23 skipped、web unit 711 件 passed、test-ts-verify 20 passed）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- migration の試験: `cargo nextest run -p task-core` 1260 件 passed、`project_evidence_integrity` 6 件 passed（migration 0119・schema 119 への付け替え後）

## 本番への移行
migration 0119 は index 2 本の追加のみ（データ変更なし）。config の変更は無い。main の 0118（N5、evidence_integrity）の後に適用される。
