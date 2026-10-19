---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: API試験所有RunContext literalを同期（api-worker-callsite）
work_unit: api-worker-callsite
status: done
updated: 2026-10-19
---
# api-worker-callsite

- task-api の RunContext literal は `tests/browser_e2e.rs`・`tests/browser_h3_injection.rs` の 2 件のみ。どちらも `..Default::default()` 展開で既にコンパイル可能だったが、plain run の意味（pin なし）を明示するため `campaign: None` を足した。
- 実 userns/browser は動かしていない。
- 証拠: `cargo check -p task-api --all-targets` exit 0、`cargo fmt --all -- --check` exit 0。
