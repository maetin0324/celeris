---
title: P4-C blocked の task が多いときの dispatch の遅延の改善
tasks: [01M6PAMC3PDP1DR5MR0W5ZRV0E]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-C blocked の task が多いときの dispatch の遅延の改善

完了日: 2026-11-05。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。
ADR は `agent-docs/adr/2026-11-05-phase4-p4c-scoped-inbox.md`。

## 結論
- **blocked 600 件の dispatch の遅延 p50 は off 3,617 → 272 ms（13.3 倍）、shadow 5,395 → 299 ms（18.0 倍）。目標 p50 ≤ 500 ms を満たした。**
- 2,000 件は off 21,567 → 763 ms（28 倍）、shadow 17,965 → 727 ms（25 倍）。10,000 件は after だけ測り off 3,252 ms・shadow 4,018 ms（before は 1 条件 30 分を超える見込みで未実施。P4-A でも 600 秒で終わらなかった）。
- 直したこと: CoS の一次対応（`triage_events`・`triage_fallback`）が毎 tick 全件の受信箱を作り直していたのを、触れた task の項目だけを作る `human_inbox_touching` に替えた。全件の組み立ても blocked の task の event を 4 回から 2 回の読みに減らし、`store.list` の読み直しを消した。
- 残る比例の項: 1 回の組み立てに 1 回の `store.list(None)`（10,000 件で約 1.1 秒）。10,000 件で p50 ≤ 500 ms は満たさない（「残り」）。

## 計測（同じ host・同じ command・同じ binary の作り方。before は基準の製品 code ＋この task の試験）
command（`crates/celeris` で test binary を直接起こす。nextest の 600 秒の打ち切りを避ける）:
`TICK_SCALE_RAW=<abs>/<arm>.jsonl CELERIS_SCRIPT_REQUIRE_BUNDLE=1 TICK_SCALE_COND=<off|shadow> TICK_SCALE_BLOCKED=<N> TICK_SCALE_TICKS=3 TICK_SCALE_DISPATCHES=<k> <tick_scale_baseline の binary> --exact tick_scale_baseline --nocapture`
（10,000 件は `TICK_SCALE_TICKS=1`）。raw は `p4c/before.jsonl`・`p4c/after.jsonl`（`celeris.tick-scale-baseline/1`、1 行 1 条件）。
fake harness（`sh`）・一時 DB・LLM なし。debug build。host は共用で load average 16〜38（24 core）の間に測った。

| 条件 | blocked | 標本 | before p50 / p95 / p99 / 最大（ms） | after p50 / p95 / p99 / 最大（ms） | p50 の倍率 |
|---|---|---|---|---|---|
| off | 600 | 20 | 3,617 / 3,855 / 3,855 / 4,765 | 272 / 520 / 520 / 551 | 13.3 |
| shadow | 600 | 20 | 5,395 / 8,692 / 8,692 / 9,780 | 299 / 383 / 383 / 392 | 18.0 |
| off | 2,000 | before 8・after 20 | 21,567 / 32,300 / 32,300 / 35,255 | 763 / 893 / 893 / 1,000 | 28.3 |
| shadow | 2,000 | before 8・after 20 | 17,965 / 30,861 / 30,861 / 40,527 | 727 / 855 / 855 / 3,149 | 24.7 |
| off | 10,000 | after 10 | 未実施 | 3,252 / 3,772 / 3,772 / 16,402 | - |
| shadow | 10,000 | after 5 | 未実施 | 4,018 / 4,049 / 4,049 / 4,081 | - |

- 標本が 20 以下なので p95・p99 は上位 1〜2 標本の値（同じ値になる）。
- dispatch 中の `tick_cos_chat_launch`（p50）: 600 件 off 3,533 → 188 ms、2,000 件 off 21,455 → 648 ms、10,000 件 off 2,948 ms。
- restart の最初の tick（全件の照合を含む）: 600 件 off 3,931 → 1,863 ms、2,000 件 off 15,626 → 12,012 ms、10,000 件 18,838 ms。restart の後の取り込みが終わるまで（`settle_*`）は 600 件 off 41.3 → 4.8 秒。
- 空 tick（取り込みの後）: 600 件 5〜9 ms、10,000 件 約 300 ms。

### P4-A の未解決 1（複写の空 tick が本物より遅い）の原因
複写した task の event は CoS の一次対応の取り込みの cursor（`feed_cursor` の `cos_triage:events`）より後ろにあり、restart 後の数十 tick がその取り込みと退避に使われていた。P4-A の空 tick はその途中を測っていた。試験に「cursor が最新に追いつき、`cos_inbox_items` に pending・running が無くなるまで tick する」段（`settle_ticks`・`settle_us`）を足し、その後の空 tick は 5〜9 ms（本物の対照 5 ms と同じ桁）になった。

## 守った規則（試験）
- 受信箱の項目の同値: `human_inbox_touching_equals_full_inbox_filtered_by_touched_tasks`（task-ops）。質問・木の子・失敗・途中確認・browser の前提・下書き・クラスタを混ぜ、触れた task の 12 通りの組で全件から抜いた項目と並びまで一致。
- 取りこぼし 0・二重実行 0・blocked は dispatch されない・run とリースが残らない: `tick_scale_baseline` が全計測（600/2,000/10,000 件、off/shadow）で assert（挿入した task はどれも run 1 本、blocked の数 = 複写の数 + 挿入の数、running の run 0、リース 0）。
- wait/depends_on/human gate/lease・複数案件の fairness・restart と孤児回収: scheduler・lease・孤児回収の code は変えていない。既存の試験（`dispatcher/tests` の tick_and_dispatch・task_waits・human_gates・orphan_takeover・ownerless_runs、`long_run_soak` の restart）は test-affected で走らせた（下）。
- 旧 milestones API の 410（`milestone_lifecycle_endpoints_are_gone`）は触れていない。

## 変更（commit の件名）
- `p4c: CoS の一次対応の受信箱を触れた task の分だけ作り、blocked の event の読み直しを減らす`

## 証拠（提出時の HEAD で実行）
- 下の「検査」節に書く。

## 失敗ケース
- 10,000 件の before は未実施（1 条件 30 分超の見込み。P4-A で nextest の 600 秒に収まらなかった）。
- 最初の計測（`settle` を足す前）の after 600 件 off は p50 662 ms だった。取り込みの途中を測っていたためで、raw は消して settle つきで取り直した。

## 境界の確認
- 実 LLM・有料 API・本番の DB/config/daemon・systemctl・promote・本番 shadow・24〜72 時間の稼働はしていない（未実施）。CPU を焼く負荷はかけていない（負荷は DB の行の数だけ）。
- human_required・許可・本番操作の保護、TS と Rust の分担、K2/K4/IEEE Access の凍結評価には触れていない。TS の policy・bundle は変えていない（TS だけの edit→verify の cargo 0 回は不変）。migration は使っていない（予約 0116 は未使用）。

## 未実施（人の承認が要る）
- 本番の CoS 有効の config での計測、本番 shadow、24〜72 時間の稼働。

## 未解決
1. 10,000 件で p50 3.3〜4.0 秒。残る比例の項は 1 回の組み立てに 1 回の `store.list(None)`（10,000 件で約 1.1 秒、1 dispatch に約 4 回）。
2. 計測は debug build。release build の絶対値は未計測（倍率は同じ build 同士）。
3. CoS 有効（`[cos]`）の条件は未計測。CoS 有効では退避が期限（既定 120 秒）まで走らないので、退避の組み立ての分は減る見込み（未確認）。
4. restart の最初の tick（全件の照合）は 600 件で 1.9 秒、10,000 件で 18.8 秒残る。

## 次の gate
- P4-D（wake・判断の行の肥大）は P4-C の後に着手できる。

## 提案
- 1 tick で task の一覧を 1 回だけ読み、`triage_events` と `triage_fallback` で共有する（10,000 件で約 2 秒減る見込み、未確認）。
- 範囲つきの組み立ての一覧を、触れた task とその root・親の query に寄せる（suppression 規則の同値を試験で固定してから）。
