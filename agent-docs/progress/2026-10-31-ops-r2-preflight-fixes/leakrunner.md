---
title: K2 ops-r2 の開始前チェック修正 — WU leakrunner（漏洩検査の試験の起動 script）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU leakrunner

完了日: 2026-10-31

## 内容
- `scripts/campaign/run-ops-r1-leak-tests.sh`（POSIX sh、引数なし）を新設した。自分の置かれた dir へ cd し、`python3 -m unittest test_check_ops_r1_leak` を exec する。
- 呼び出しは `sh scripts/campaign/run-ops-r1-leak-tests.sh`。どの cwd でも通る。
- checker と試験の内容は変えていない。

## 検査
- repo 直下から `sh scripts/campaign/run-ops-r1-leak-tests.sh`: 47 件 OK
- cwd を /tmp にして絶対 path で同 script: 47 件 OK、exit 0
