---
title: K2 ops-r2 の開始前チェック修正 — WU leakfix（漏洩検査の試験の起動例）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU leakfix

完了日: 2026-10-31

## 内容
- commit `leakfix: 漏洩検査の試験の起動例を cd 付き unittest に統一`。`scripts/campaign/test_check_ops_r1_leak.py` の冒頭の起動例 1 行だけを変えた。
- 旧: `python3 -I scripts/campaign/test_check_ops_r1_leak.py`（-I は cwd を sys.path に入れず、統合で再実行されると import できなかった）。
- 新: `cd scripts/campaign && python3 -m unittest test_check_ops_r1_leak`。
- 後続の WU leakrun・leakrunner で、起動は `sh scripts/campaign/run-ops-r1-leak-tests.sh` に統一された。起動例はこの run script の形で書く。
- checker・試験の内容は変えていない。

## 検査
- 起動例の 1 行のみの変更。試験の実行結果は WU leakrun・leakrunner の記録（47 件 OK）を参照。
