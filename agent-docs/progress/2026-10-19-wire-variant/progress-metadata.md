---
title: 既存 progress 4 件の front matter 修復（progress-metadata）
tasks: [01M59P79S6NTGRN3X7SQDV9C0G]
status: done
updated: 2026-10-19
---
# 既存 progress 4 件の front matter 修復

`sh scripts/dev/progress-index.sh --check` が落ちていた 4 件に title / tasks / updated を補った。本文は変更していない（`git diff` は front matter 行の追加のみ、10 行追加・削除 0）。

| ファイル | title の出典 | tasks の出典 | updated の出典 |
|---|---|---|---|
| 2026-10-18-cap-env-content-pin.md | 本文の題名 | 元 task 不明（git 履歴に task ID 無し）。この修復 task 01M59P79S6NTGRN3X7SQDV9C0G を関連 task として置く。元 task であるとは主張しない | 既存 completed 2026-10-18 |
| 2026-10-18-resolved-routing-evidence-mode.md | 本文の題名 | 既存 `task: 01M57DXA25E8KB5FC75FN0TXY7`（既存行は残した） | 既存 completed 2026-10-18 |
| 2026-10-18-result-location-scope.md | 本文の題名 | 既存 `tasks:` を維持 | 既存 completed 2026-10-18 |
| 2026-10-19-cap-probe-ro-mount-kind.md | 本文の題名 | 既存 `tasks:` を維持 | 既存 completed 2026-10-19 |

## 証拠
- 修復前: `sh scripts/dev/progress-index.sh --check` → exit 1（上記 4 件の title/tasks/updated 違反）。
- 修復後: 同コマンド → `progress-index --check: ok`、exit 0。
- 検査 script は変更していない。Rust 変更なし。
