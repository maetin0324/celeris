---
title: "cargo fmt --check の必須化と operator script WU の拒否"
tasks: [01M51B9AFERE4NH2XXQG8B4WRM]
status: done
completed: 2026-10-16
updated: 2026-10-16
---
# cargo fmt --check の必須化と operator script WU の拒否

ADR: `agent-docs/adr/2026-10-16-fmt-check-and-no-operator-script-units.md`

## 証拠
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0（passed 5373 / failed 0 / ignored 13。clippy 修正前の実行。修正は検証関数の split 引数のみで、`cargo test -p task-core execution_plan` 92 passed で再確認）

## 未解決事項
なし。objective 本文に release.sh と書く unit は検査しない（check の実行形だけを決定的に見る）。
