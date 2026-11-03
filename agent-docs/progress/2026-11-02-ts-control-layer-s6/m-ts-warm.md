---
title: S6 m-ts-warm — policy-ts・cos-ts の warm を各 20 試行
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: running
updated: 2026-11-03
---
# S6 m-ts-warm — policy-ts・cos-ts の warm を各 20 試行

[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従う。コードと script は変えていない。

## 開始前の状態（2026-11-03 11:26 JST ごろ、host home-dev）
- `uptime` → `up 18 days, 20:39, 3 users, load average: 7.96, 11.27, 8.38`（24 CPU）
- 他の cargo・rustc: `pgrep -c -x cargo` → 0、`pgrep -c -x rustc` → 0
- `git status --porcelain` は空（この file の commit の後に測る）
