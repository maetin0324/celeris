---
title: "全体試験の再確認"
tasks: [01M4J531V95EJXV8GXTXTWH6SY]
status: done
updated: 2026-10-10
---

# 全体試験の再確認

## 結果

final review で報告された `5041 passed / 2 failed` は、この作業ツリーでの再実行では再現しなかった。失敗名は元の review 報告に含まれず、再現ログにも失敗がないため「再現せず」と記録する。原因は特定できず、branch 起因を示す証拠も得られなかった。コード・試験の修正は行っていない。

| 全体試験 | exit | nextest passed / failed | ignored | doctest |
|---|---:|---:|---:|---|
| 1回目 (`tp1.log`) | 0 | 5043 / 0 | 14 | exit 0 |
| 2回目・最終 (`tp2.log`) | 0 | 5043 / 0 | 14 | exit 0 |

失敗試験名: 再現せず（元の final review では2件の名前が不明）。失敗時の `FAIL [` 行は両ログとも 0 件。sandbox 既知失敗（browser・launcher・credentiald・CDP・userns）も再現しなかった。

## 環境と検査

- `df -h /local`: 300G 中 199G 使用、98G 空き、68%。95% 未満のため disk_watch は原因ではない。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。

ログはリポジトリ外の `/local/celeris/data/workspaces/01M4J531V95EJXV8GXTXTWH6SY/wu/reverify-full/artifacts/tp1.log` と `tp2.log` に保存した。
