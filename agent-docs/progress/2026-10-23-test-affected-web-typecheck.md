---
title: test-affected.sh で web の型検査を走らせ、release gate で初めて web typecheck が落ちるのを防ぐ
updated: 2026-10-23
completed: 2026-10-23
tasks: [01M5MQ6SYDXFY1QCGN8HWENWMT]
status: done
---
# test-affected の web 型検査

ADR: `agent-docs/adr/2026-10-23-test-affected-web-typecheck.md`

## 変更

- `scripts/dev/test-affected.sh`: web/・docs/api/・workspace 共通 file・task-api とその通常依存の crate の変更で
  web の install（node_modules が無いとき offline）→ typecheck（tsc -b）→ unit 試験を cargo より先に走らせる。
  crates の差分があっても省かない。`--list` に `web: typecheck+unit (理由: …)` の行。
- `scripts/dev/tests/test-affected-list.sh`: (e) 選択、(f) 実行（偽の pnpm・tsc・vitest）を追加。
- `CLAUDE.md`・`agent-docs/guides/testing.md` の test-affected の説明。

## 証拠

- `sh scripts/dev/tests/test-affected-list.sh` → `test-affected-list: ok`（exit 0）
- 実 repo で task-core に 1 行足して `--list` → 全 13 crate＋`web: typecheck+unit (理由: web の型の元になる crate の変更: task-core)`。
  routing-eval だけの変更では web 行なし。
- 実 worktree（node_modules 無し）で `pnpm install --offline --frozen-lockfile` 0.5 秒、`tsc -b` 1.4 秒。
- 実 repo で web/ に一時 file を置いて実行 → exit 0（vitest 682 passed、node --test 98 pass）。
  invalidation-map.ts から `required_checks_evaluated` の行を消して実行 → exit 1、`error TS2741: Property 'required_checks_evaluated' is missing … Record<EventKind, KindSpec>`（10/23 の release gate と同じ失敗）。
- `bash scripts/dev/test-affected.sh --list` exit 0（出力なし: 変更は scripts・docs だけ）、`bash scripts/dev/test-affected.sh` exit 0、
  `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決事項

- なし。

## 提案

- `scripts/dev/tests/test-affected-list.sh` は check-health.sh から呼ばれていない（手動）。health gate に入れると
  test-affected.sh 自身の退行を全 task で捕まえられる（数秒）。
