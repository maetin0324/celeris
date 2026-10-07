---
title: ビルド成果物と /tmp の残骸が際限なく溜まる問題の再発防止
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
status: running
updated: 2026-10-07
---
# ビルド成果物と /tmp の衛生

ADR `agent-docs/adr/2026-10-07-build-tmp-hygiene.md`（状態: 採択・実装前）。葉ごとの進捗は `2026-10-07-build-tmp-hygiene/<key>.md`。

## 葉一覧

| 葉 | 状態 |
|---|---|
| adr | 完了（2026-10-07） |
| main-sync / repair-design-1 | 完了（2026-10-07） |
| run-tmpdir / rust-test-tmp / target-sweep / web-e2e-tmp | 未着手 |
| disk-watch | 未着手 |
| close-out | 未着手 |

## 未解決事項

- remote run の TMPDIR は範囲外（ADR §4）。
- test-parallel.sh・clippy の結果は実装の葉と close-out で記録する。
