---
title: K2 ops-r2 の開始前チェック修正 — WU leakrun（漏洩検査の試験の起動例）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU leakrun

完了日: 2026-10-31

## 内容
- 原因: 起動例が `python3 -I` で試験を起動していた。-I は cwd を sys.path に入れず、`test_check_ops_r1_leak` を import できない。
- `scripts/campaign/check-ops-r1-leak.py` 冒頭に試験の起動例 `cd scripts/campaign && python3 -m unittest test_check_ops_r1_leak` を載せた（checker 本体の起動例は -I のまま）。
- `scripts/campaign/test_check_ops_r1_leak.py` は自分のある dir を sys.path の先頭に足し、直接実行でも通る。
- checker の判定規則は変えていない。docs/ops は触っていない。

## 検査
- `cd scripts/campaign && python3 -m unittest test_check_ops_r1_leak`: 47 件 OK
- `python3 scripts/campaign/test_check_ops_r1_leak.py`: 47 件 OK
