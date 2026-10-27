---
title: promote・rollback で credentiald を切り替える
updated: 2026-10-27
status: done
tasks: [01M5XXF7QNDDFANAXYRND6T9VY]
---

# promote・rollback で credentiald を切り替える

- 実装: `sd_switch_credentiald`（lib.sh）、promote.sh（2 経路）、rollback.sh。ADR 2026-10-27-promote-switches-credentiald。
- 試験: `tests/credentiald_switch.sh`。既存の promote 試験の偽 systemctl は credentiald の is-active を真にした。
- 未解決: 本番での確認は次の昇格で operator が行い、ここに残す。

## 本番への移行
config の変更は無い。次の昇格で `systemctl --user status celeris-credentiald@<新>` が active なことを確かめる。
