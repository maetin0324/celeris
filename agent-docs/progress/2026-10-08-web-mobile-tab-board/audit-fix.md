---
status: done
completed: 2026-10-08
---
# audit-fix: mobile-audit のタップ領域違反を直す

mobile-audit は input 要素そのものの box を測るので、label を広げても足りない。checkbox 自体を size-11（44×44px）にした。

| 画面 | 要素 | before | after |
|---|---|---|---|
| /models（model-role-editor.tsx）| 役割割当 checkbox（tier ごと）| 20×20 | 44×44（size-11）|
| /models（model-role-editor.tsx）| 優先度 number input | 40×44（flex で縮む）| w-16 shrink-0 で 64×44 |
| /models（models-screen.tsx）| 無効化 checkbox | 20×20 | 44×44 |
| /browser/settings（browser-settings-screen.tsx）| click・download 承認 checkbox | 20×20 | 44×44（li の gap を 3→1）|

## 証拠
- `pnpm -C web build` → exit 0。`pnpm -C web mobile-audit` → `38 path(s) x 4 widths ok`、exit 0（修正前は exit 1）
- `pnpm -C web lint` / `typecheck` / `test` → すべて exit 0
- `pnpm -C web e2e`（functional）→ 313 passed, 8 skipped, exit 0
- crates/ の差分なし
