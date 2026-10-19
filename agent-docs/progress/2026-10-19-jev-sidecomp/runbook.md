---
title: "docs/ops/jev-c1-live-check.md に実検証手順を反映"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: runbook
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# runbook

- §4 を `jev-live-evidence.py` に合わせて書き換え（旧「手作業の抽出」記述を削除）。B1/C1 別測定 ID・条件 hash 固定と拒否・累計予算（4.1）、正確な呼出し行・export への添付 file・終了コード 0/1/2（4.2）、live 合格条件（4.3）、手組み fixture が合格にならないこと（4.4）。
- 検証器の docstring・`--help`・provenance 必須欄と突き合わせた。worker は検証器を実 export で実行していない（人が実行）。効果・有意性は書いていない。
- 未解決: export が provenance.json / routing_events.jsonl を出さない（人が添える）。実 daemon の live export は未入手。
