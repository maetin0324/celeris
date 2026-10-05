---
title: record-r5 — after-r4 目視判定の統合
tasks: [01M45TA2085GTJVFNFR8CQ1G43]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# record-r5 — after-r4 目視判定の統合

[web-ux 本文](../../2026-10-04-web-ux.md)を after-r4 基準に書き直した。画像別の一次記録は [look-a](look-a.md)・[look-b](look-b.md)・[look-c](look-c.md)。撮影と修正は [fix-r5](../fix-r5.md)、検査は [gates-r5](../gates-r5.md)。after-r4 は `/local/celeris/data/workspaces/01M45PRPACBEP4X1FWPNP4FRMG/wu/fix-r5/artifacts/after-r4`、before は `/local/celeris/data/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before` にある。

| 区分 | 見た after-r4 | 見ていない after-r4 | 判定 |
| --- | ---: | ---: | --- |
| 基本 31 画面 × 4 幅 | 124（look-a 64・look-b 60） | 0 | 可 98・要修正 7・保留 19 |
| 状態変種 29 組 × 4 幅 | 58（look-c、1440・360） | 58（390 が 29、412 が 29） | 可 40・要修正 6・保留 12 |
| 合計 | **182/240** | **58/240** | **可 138・要修正 13・保留 31** |

reviewer の (a) `_artifacts-360.png`、(b) `_projects_P1-360.png`・`_projects_P1-390.png`、(c) `error-_inbox-1440.png` など error 4 画面 × 1440・360、(d) `_-1440.png` は、指摘された現象の解消を画像で確認した。判定は指摘の条件に限る。画面全体には狭幅の情報の切れ、空・権限・stale 状態の案内、正常 fixture が無いための保留が残る。

[gates-r5](../gates-r5.md) は HEAD `a9d757b1` で **13 本中 12 本 exit 0、S1 latency 1 本 exit 1**。S1 は `/reports` の heading 10s−0s が 116.56ms、`/approvals` の URL 10s−0s が 140.94ms で、ともに 100ms の閾値を超えた。再実行はなく、全検査合格の判定は出していない。

本文の対応表は基本 124 枚を after-r4 のファイル名へ置き換え、before が無い通知 4 枚と、before を目視していない幅を区別した。状態変種の対応と未見の 58 枚は look-c の記録に委ねた。`web/`・`crates/` は編集していない。
