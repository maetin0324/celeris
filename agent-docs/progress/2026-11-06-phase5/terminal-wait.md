---
title: 第5フェーズ closeout の前提 — N0〜N7 の正本の終端待ち（terminal-wait）
tasks: [01M6QW0ANN9CS5CWHMDP3N6PKF]
status: done
updated: 2026-11-06
---
# N0〜N7 の正本の終端待ち

## 結論
2026-11-06T15:18:15Z の正本照会で N0〜N7 の 8 件はすべて `done` になった。結果を `closeout-terminals.json` に書いた。N7（01M6QW0GT56KDVBHCXX78GBFHF）は updated_at 2026-11-06T15:17:31.176723065Z で `done`。

経緯: 1 回目の待ち（14:03:08Z 開始）は上限 3000 秒までに N7 が終端にならず、JSON を作らずに終えた。2 回目の待ち（14:53:57Z 開始、上限 15:43:57Z）で 15:18:11Z に全件終端になり、15:18:15Z にもう 1 回照会して確かめた。N7 は以前に done から reviewing へ戻ったことがある（14:23Z→14:25Z）。`closeout-terminals.json` は 15:18:15Z 時点の記録で、その後の状態変化はこの文書の範囲外。

総括・main 同期・本番変更・有償 LLM・soak はこの WorkUnit では行っていない。

## 照会の方法
- 照会元: `/local/celeris/data/db/celeris.sqlite3`（正本）。`mode=ro` の URI で開き、`PRAGMA query_only=ON`、`BEGIN`〜`COMMIT` の単一 transaction で `SELECT id,status,updated_at FROM tasks WHERE id IN (8 件)` を実行した。書き込みはしていない。
- 間隔: `sleep 120` で 120 秒ごと。1 回の Bash は 4 照会（約 8 分）に区切った。
- 終端は `done`・`failed`・`cancelled`。進捗文書の status は使っていない。

## 最終の照会（2026-11-06T15:18:15Z、closeout-terminals.json と同じ値）

| task | id | status | updated_at |
|---|---|---|---|
| N0 | 01M6QW0GSCHZZGYRNQMNRKRDZA | done | 2026-11-06T07:58:33.861377708Z |
| N1 | 01M6QW0GSG1AVARWC334AZCES1 | done | 2026-11-06T13:40:03.769188271Z |
| N2 | 01M6QW0GSJR1X7797YF4Y1HWR8 | done | 2026-11-06T11:03:08.585313147Z |
| N3 | 01M6QW0GSM7ZHFNP3PWZJJ4SNF | done | 2026-11-06T08:29:00.846168876Z |
| N4 | 01M6QW0GSR42EYSKBME8VJ5TDF | done | 2026-11-06T12:10:50.755012464Z |
| N5 | 01M6QW0GSWRGP85RE263TWTM01 | done | 2026-11-06T11:41:13.425832841Z |
| N6 | 01M6QW0GT1GBT4M4QKDDG79Z7X | done | 2026-11-06T14:40:02.186616155Z |
| N7 | 01M6QW0GT56KDVBHCXX78GBFHF | done | 2026-11-06T15:17:31.176723065Z |

## 照会の経過（UTC）

### 2 回目の待ち（14:53:57Z 開始、上限 15:43:57Z）
Bash 1 回あたり 4 照会（約 8 分）に区切り、`sleep 120` で待った。各照会の全件出力は run の成果物 `terminal-wait-log.jsonl` に残した。

| 照会時刻 | N7 | 非終端の task |
|---|---|---|
| 2026-11-06T14:53:57Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:56:04Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:58:04Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:00:04Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:02:05Z | reviewing | 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T15:04:07Z | reviewing | 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T15:06:07Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:08:07Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:10:08Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:12:10Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:14:11Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T15:16:11Z | reviewing | 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T15:18:11Z | done | なし |
| 2026-11-06T15:18:15Z | done | なし |

### 1 回目の待ち（前回の run、14:03:08Z 開始、上限 14:53:08Z で打ち切り）
N6 は 14:08:33Z に done から running へ戻り、14:31Z ごろから reviewing、14:40:02Z に再び done になった。N7 は 14:21Z〜14:23Z の照会で done だったが、14:25Z には reviewing に戻り、14:41Z 以後は running。終端は確定していないものとして扱う。

| 照会時刻 | N7 | 非終端の task |
|---|---|---|
| 2026-11-06T14:03:08Z | reviewing | 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:05:18Z | reviewing | （未記録） |
| 2026-11-06T14:07:18Z | reviewing | （未記録） |
| 2026-11-06T14:09:19Z | reviewing | （未記録） |
| 2026-11-06T14:11:19Z | reviewing | （未記録） |
| 2026-11-06T14:13:22Z | reviewing | （未記録） |
| 2026-11-06T14:15:22Z | reviewing | （未記録） |
| 2026-11-06T14:17:22Z | reviewing | （未記録） |
| 2026-11-06T14:19:23Z | reviewing | （未記録） |
| 2026-11-06T14:21:25Z | done | （未記録） |
| 2026-11-06T14:23:25Z | done | （未記録） |
| 2026-11-06T14:25:26Z | reviewing | （未記録） |
| 2026-11-06T14:27:26Z | reviewing | （未記録） |
| 2026-11-06T14:29:35Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X running, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:31:35Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X reviewing, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:33:35Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X reviewing, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:35:35Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X reviewing, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:37:37Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X reviewing, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:39:37Z | reviewing | 01M6QW0GT1GBT4M4QKDDG79Z7X reviewing, 01M6QW0GT56KDVBHCXX78GBFHF reviewing |
| 2026-11-06T14:41:38Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:43:38Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:45:40Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:47:40Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:49:41Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |
| 2026-11-06T14:51:41Z | running | 01M6QW0GT56KDVBHCXX78GBFHF running |

注: 14:05〜14:27Z の照会では非終端の一覧を記録していなかった（N7 の status だけ）。N6 の非終端は 14:27Z の照会の全件出力で見つけ、以後の照会から一覧を記録した。14:03:08Z の照会の直後にも同じ照会を 1 回実行した（結果は記録していない）。

### 1 回目の待ちの最後の照会（2026-11-06T14:51:41Z）

| task | id | status | updated_at |
|---|---|---|---|
| N0 | 01M6QW0GSCHZZGYRNQMNRKRDZA | done | 2026-11-06T07:58:33.861377708Z |
| N1 | 01M6QW0GSG1AVARWC334AZCES1 | done | 2026-11-06T13:40:03.769188271Z |
| N2 | 01M6QW0GSJR1X7797YF4Y1HWR8 | done | 2026-11-06T11:03:08.585313147Z |
| N3 | 01M6QW0GSM7ZHFNP3PWZJJ4SNF | done | 2026-11-06T08:29:00.846168876Z |
| N4 | 01M6QW0GSR42EYSKBME8VJ5TDF | done | 2026-11-06T12:10:50.755012464Z |
| N5 | 01M6QW0GSWRGP85RE263TWTM01 | done | 2026-11-06T11:41:13.425832841Z |
| N6 | 01M6QW0GT1GBT4M4QKDDG79Z7X | done | 2026-11-06T14:40:02.186616155Z |
| N7 | 01M6QW0GT56KDVBHCXX78GBFHF | running | 2026-11-06T14:40:55.532199475Z |
