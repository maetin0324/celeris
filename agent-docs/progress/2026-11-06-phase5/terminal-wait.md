---
title: 第5フェーズ closeout の前提 — N0〜N7 の正本の終端待ち（terminal-wait）
tasks: [01M6QW0ANN9CS5CWHMDP3N6PKF]
status: blocked
updated: 2026-11-06
---
# N0〜N7 の正本の終端待ち

## 結論
待ちの上限（開始 14:03:08Z から 3000 秒、14:53:08Z）までに 8 件は全件終端にならなかった。最後の照会（14:51:41Z）で N7（01M6QW0GT56KDVBHCXX78GBFHF）は `running`（updated_at 14:40:55Z）。N0〜N6 の 7 件は `done`。このため `closeout-terminals.json` は作っていない。総括・main 同期は始めていない。

待ち対象: N7 01M6QW0GT56KDVBHCXX78GBFHF（running）。

## 照会の方法
- 照会元: `/local/celeris/data/db/celeris.sqlite3`（正本）。`mode=ro` の URI で開き、`PRAGMA query_only=ON`、`BEGIN`〜`COMMIT` の単一 transaction で `SELECT id,status,updated_at FROM tasks WHERE id IN (8 件)` を実行した。書き込みはしていない。
- 間隔: `sleep 120` で 120 秒ごと。1 回の Bash は 4 照会（約 8 分）に区切った。
- 終端は `done`・`failed`・`cancelled`。進捗文書の status は使っていない。

## 最後の照会（2026-11-06T14:51:41Z）

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

## 照会の経過（UTC）
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

## 次の手
N7 が終端（done・failed・cancelled）になった後に、この照会をやり直して `closeout-terminals.json` を作る。
