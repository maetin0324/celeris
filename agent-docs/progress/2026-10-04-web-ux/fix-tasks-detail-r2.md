---
title: /tasks/:id の判断節・実行節・概要節に残る enum 生値を和名にする（WU fix-tasks-detail-r2）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# /tasks/:id の判断節・実行節・概要節に残る enum 生値を和名にする

final review 不合格点 (4)「`_tasks_T1-1440` の『判断』節に生値『状態 ready』が残っている」の修正。

## 直した箇所

- `web/features/tasks/decision-panel.tsx`: 判断節の状態表示を `detail.task.status` の生値から `statusView(status).label` へ（`data-status` 属性は raw 値のまま残す）。
- `web/features/tasks/execution-panel.tsx`: 実行節の「実行の段階」（`ExecutionPhase`）と「形の判定」（`ExecutionMode`）の表示を、写像表 `phaseLabel`/`modeLabel` + `enumLabel()`（未知値は「未確認」、`status-badge.tsx` と同じ扱い）で和名化。表示は `<span data-phase=…>`/`<span data-mode=…>` に包み、raw 値は属性に残す。
- `web/features/tasks/overview-view.tsx`: 概要節の「状態」欄（facts の先頭行）も同様に `task.status` の生値表示を `statusView(status).label` に直した（判断節・実行節と同じ種類の退行だったため、同じ節の範囲内の修正として合わせて直した）。`unit.phase ?? unit.spec.phase`（段の名前）は planner が決める自由文字列で enum ではないため対象外。

## 人の決定に基づく範囲外の追加修正（decisions.parity-raw-enum-assertions）

和名化により `web/e2e/parity/task-detail.spec.ts` の 3 本（4 箇所）が raw 値を assert していて落ちる問題を、記録済みの人の決定どおりに直した（決定: 「spec を直してよい。表示の和名に依存させず、data-status 等の安定した属性で raw 値を確かめる形にする。和名の表示そのものは 1 本で確かめる。直すのは task-detail.spec.ts の該当箇所だけ」）。

- L157: `decision-status` の `toHaveText("done")` → 和名表示そのものを確かめる 1 本として `toHaveText("完了")` に変更（コメントを添えた）。
- L204 / L218: `execution-view` の `toContainText("awaiting_human"/"executing")` → `locator("[data-phase='…']")` の可視性で確認。
- L315: `decision-status` の `toHaveText("reviewing")` → `toHaveAttribute("data-status", "reviewing")` で確認。

この 1 ファイルのみ `web/features/tasks/` の外。他は一切触っていない（`git status --short` で確認）。

## 検査

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` — exit 0
- `corepack pnpm@12.6.0 -C web typecheck` — exit 0
- `corepack pnpm@12.6.0 -C web lint` — exit 0（既存の 5 warning のみ、変更箇所に起因しない）
- `corepack pnpm@12.6.0 -C web test` — exit 0, tests 42 / pass 42
- `corepack pnpm@12.6.0 -C web build` — exit 0
- `corepack pnpm@12.6.0 -C web check:boundaries && check:parity && check:secrets && gen:types --check && mobile-audit` — exit 0（mobile-audit: 31 path × 4 widths ok）
- `corepack pnpm@12.6.0 -C web e2e parity/task-detail.spec.ts` — exit 0, 7 passed
- `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'` — exit 0, 177 passed / 8 skipped（既存と同数）

## 残課題

- なし（このWUの範囲内）。全画面の視覚判定・dark mode 提案は `ux-record`/`record-r2` の記録に委ねる。
