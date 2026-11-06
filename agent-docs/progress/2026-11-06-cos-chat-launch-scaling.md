---
title: tick_cos_chat_launch の行数比例の調査と修正
tasks: [01M6QZ6KYWW18GYMZYM17SYSW9]
status: done
updated: 2026-11-06
---
# tick_cos_chat_launch の行数比例の調査と修正

## 結論
tick_cos_chat_launch の行数比例は 2 つあった。主因は relay_rejudge の候補 SQL で、副因は claim_queued・control_hook の全 thread 走査である。両方を直した。
- 主因（付記 D3-c）: `cos_triage_relay_candidates` は escalated 項目ごとに、その thread で判断より後の message 全部に `json_type(metadata_json,'$.relay')` を評価していた。費用は「escalated 項目数 × thread の message 数」に比例する。本番 backup の写しでは escalated 78 件 × 後続 message で 115,402 行を読み、候補 0 件でも relay_rejudge が 72〜213 ms かかって、tick_cos_chat_launch の 8 割強を占めた。部分 index `idx_chat_messages_relay` を足し、0.7〜1.7 ms になった。
- 副因（D3）: `claim_queued`・`control_hook` が毎 tick `chat_thread_list` で全 thread を走査していた。部分 index の列挙に替えた。
- 付記 D3-b: relay_rejudge は候補があるとき全 task の派生受信箱を作っていた。候補の task に触れる受信箱だけを作るように変え、待ちが閉じた候補は決着済みとして記録する。
設計は ADR `agent-docs/adr/2026-11-06-cos-chat-launch-scaling.md`。付記 D3-b・D3-c も、それぞれ実装より先に commit した。

## 変更（commit は件名）
- n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測（`cos.*` の段別 `step_us`）
- n2: queued 入力と実行中 run の thread だけを引く（全 thread 走査の削除・migration 0119）
  - `chat_thread_ids_with_queued_or_open_run()` を追加。部分 index `idx_chat_messages_queued`・`idx_chat_runs_open` だけを引く。
  - `claim_queued`・`control_hook` はその id だけを `chat_thread_get` で読む。旧い全 thread 走査は両方から削除した。
- n2: relay_rejudge は候補の task に触れる受信箱だけを作り、待ちが閉じた候補を決着済みにする（付記 D3-b。全件受信箱の経路は task を引けない key のときだけ残る）
- n2: 取り次ぎの候補 SQL を部分 index idx_chat_messages_relay で引く（migration 0119、付記 D3-c）
  - SQL は `RELAY_CANDIDATE_SQL` 定数 1 か所にまとめ、index の WHERE と同じ語で書く。index 無しの旧い経路は残っていない。
- 回帰試験 `thread_ids_with_queued_or_open_run_uses_partial_indexes_and_ignores_idle_rows`（task-core）。
  idle thread 300 件＋queued 1 件で、結果が queued の thread を含み、`EXPLAIN QUERY PLAN` が 2 つの部分 index を使うことを固定する。時計・負荷は使わない。

## 行数と費用の対応: 取り次ぎの候補 SQL（主因、付記 D3-c。受け入れ条件 3・4）
試験 `relay_candidate_cost_is_flat_in_thread_messages_with_the_relay_index`（task-core、`chat::store_tests`）は、escalated 項目 20 件の thread に取り次ぎではない message を 100 → 400 → 1600 件足し、同じ SQL の VM 命令数を index ありと index なし（同じ行で index を DROP して SAVEPOINT で戻す＝修正前の経路）で数える。`EXPLAIN QUERY PLAN` が `idx_chat_messages_relay` を使うことも固定する。時計・負荷は使わない。

| thread の message 数 | 修正前（index なし）の VM 命令数 | 修正後（index あり）の VM 命令数 |
|---|---|---|
| 100 | 20,536 | 532 |
| 400 | 80,536 | 532 |
| 1,600 | 320,536 | 532 |

修正前は message 1 件 × 項目 20 件あたり約 10 命令で、項目数 × message 数に比例する。修正後は一定である。本番 backup の写しに対する sqlite3 CLI（読み取り専用の別の写し）でも、同じ SQL は 0.93 秒（115,402 行）から 1 ms 未満になった。

## 行数と費用の対応: 全 thread 走査（副因、D3。受け入れ条件 3・4）
試験 `queued_thread_enumeration_cost_is_flat_while_full_scan_grows_with_rows`（task-core、`chat::store_tests`）が、idle thread（1 thread あたり message 3 件）を 100 → 400 → 1600 に増やし、SQLite の VM 命令数（`sqlite3_stmt_status(VM_STEP)`、時計に依らない）を数える。負荷・sleep・時計は使わない。

| idle thread 数 | 旧い全 thread 走査（THREAD_SELECT）の VM 命令数 | 新しい列挙（部分 index）の VM 命令数 |
|---|---|---|
| 100 | 9,208 | 25 |
| 400 | 36,508 | 25 |
| 1,600 | 145,708 | 25 |

旧い走査は 1 thread あたり約 91 命令で、thread 数に比例する。新しい列挙は 25 命令で一定。試験は「旧い走査が 8 倍以上に増える」「新しい列挙は +20 命令以内」「新しい列挙は旧い走査の 1/20 未満」を固定する。
本番規模（chat_threads 227）に当てはめると旧い走査は約 2 万命令で、数ミリ秒の量にとどまる。本番の 18〜23 秒をこの走査だけでは説明できない（下の未解決）。

## 未測定・未解決
- soak での dispatch 遅延（ready の待ち時間）の before/after は測っていない。1 時間を超える soak は起こさない約束で、この run の予算（3600 秒）では 1 時間の soak も検査と両立しない。代わりに下の表で、dispatch の周期を決める tick 全体の所要時間（total）と dispatch_ready 段を写しで比べた。soak の傾きが消えたことは release 後の soak か本番の slow tick の `cos.*` で確かめる必要がある（worker は release・promote をしない）。
- triage_ingest は本番規模の写しで tick 1 以降 6〜24 ms（tick 0 のみ 1.8〜5.1 秒）で、遅い段ではなかった。root_done_tick も 1 ms 未満だった。行数比例かは決定的な試験では調べていない。
- RSS 増加 task 01M6QZ031RC9YK3E6T12ESCR80 とは重複して直していない。この変更は SQL の読む行数を減らすだけで、arena の設定には触れていない。ただし修正前の候補 SQL は毎 tick 約 11 万行の metadata_json を読んで JSON を評価していたので、一時の確保が減る効果はありうる（未確認）。

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

## 検査結果（提出時の HEAD で実行。最後の同期（main 取り込み）の後、commit「n2: 主因（取り次ぎの候補 SQL）の特定と before/after を進捗に書く」の時点）
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 7765 件 passed・23 skipped、web unit 711 件 passed）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 主因（付記 D3-c）の before/after: 本番 backup の写し、交互に 2 組
手順は上と同じ（`tick_prod_copy_steps`、backup `20261106-093409-pre-c9bf4bf7dc76.sqlite3` を毎回新しく一時 dir へ複製、fake harness だけの config）。修正前は commit「n2: relay_rejudge は候補の task に触れる受信箱だけを作り、待ちが閉じた候補を決着済みにする」の時点（D3・D3-b は入り、D3-c は無い）、修正後は commit「n2: 取り次ぎの候補 SQL を部分 index idx_chat_messages_relay で引く（migration 0119）」の時点。修正前 → 修正後 → 修正前 → 修正後 の順に走らせた。tick 1〜4 の 8 tick（各版）の最小・中央値・最大。単位は ms。写しは測定後に消した。

| 段 | 修正前 最小 / 中央 / 最大 | 修正後 最小 / 中央 / 最大 |
|---|---|---|
| tick_cos_chat_launch | 91.4 / 174.6 / 257.8 | 11.3 / 13.0 / 16.0 |
| cos.relay_rejudge | 72.2 / 148.3 / 212.7 | 0.7 / 0.7 / 1.7 |
| cos.claim_queued | 1.0 / 1.1 / 7.3 | 0.9 / 1.0 / 3.5 |
| cos.control_hook | 3.7 / 9.2 / 17.4 | 2.4 / 2.5 / 4.0 |
| cos.triage_ingest | 9.7 / 15.7 / 24.1 | 6.5 / 6.8 / 10.5 |
| dispatch_ready | 753.8 / 1,152.2 / 5,072.3 | 664.0 / 737.8 / 959.9 |
| recover_reviews | 2,708.9 / 4,528.2 / 7,492.1 | 1,813.0 / 2,157.5 / 6,751.1 |
| prune_and_sweep | 648.1 / 992.8 / 1,817.3 | 451.5 / 635.6 / 793.8 |
| tick 全体（total） | 4,827 / 8,241 / 10,390 | 3,046 / 3,648 / 8,572 |

読み取り:
- tick_cos_chat_launch の中央値は 174.6 ms から 13.0 ms に下がった（約 1/13）。下がった分のほぼ全部が relay_rejudge（148.3 ms → 0.7 ms）である。
- 他の段は修正後に悪化していない。dispatch_ready・recover_reviews・prune_and_sweep は修正後のほうが小さいが、これらの段は修正に触れていないので、差は host の揺れと見る。
- dispatch の周期を決める tick 全体は中央値 8.2 秒 → 3.6 秒だった。差の大半は recover_reviews など修正対象外の段の揺れで、修正による短縮は tick あたり約 0.16 秒である。
- 本番の 18〜23 秒（09:50〜10:10Z）は写しでは再現しない。ただしこの SQL は cache が冷えた状態で 1 回 0.93 秒かかり（sqlite3 CLI）、escalated 項目と inbox thread の message が増えるほど伸びる。soak の傾き（escalated 項目と message が単調に増える）と矛盾しない説明だが、soak・本番での確認は未実施である。
- 生データ: run の artifacts の `m-{1,2}-{after,d3c}.jsonl`（repo には入れない）。
