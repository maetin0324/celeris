---
task: 01M4ZRKYMX6FWH0VCWY3GXPX94
wu: jev-e2e
status: done
completed: 2026-10-15
---
# jev-e2e: fake server 一連試験と進捗『本番への移行』

- `crates/celeris/src/daemon/classify_tick/e2e.rs`（試験 2 件）: 未登録 → credential 待ち → 通知 → 鍵登録 → 同じ WU が一度だけ再開 → 分類（Jev へ 1 回）、429 は待ちにならない。詳細・証拠は `../2026-10-15-jev-wire.md`。
- 「本番への移行」節（config 差分・鍵登録手順）は `../2026-10-15-jev-wire.md` に書いた。release・verify・昇格・本番 config・鍵登録は行っていない。
