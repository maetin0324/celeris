---
title: 同梱の検査器の process 共有 static で落ちる 11 件を cargo test でも通す（fix-cargo-test）
tasks: [01M79DR235G75X2JKTWA794R7T]
status: done
updated: 2026-11-13
---

# fix-cargo-test 進捗

完了日: 2026-11-13

## 原因と直し方
- 原因: `gate.rs` の `static BUNDLED`（`OnceLock`）を、stub を設定する試験と未設定を assert する試験が共有していた（[verify-nextest.md](verify-nextest.md)）。
- 本番のコードは変えていない。`#[cfg(not(test))]` では従来の `OnceLock` のままで、fallback・切替 flag は足していない。
- 試験側: `gate::bundled_test`（`#[cfg(test)]`）に保管場所を置き、試験が `Kind`（`Stub`／`Unset`／`Custom`）を宣言する。違う種類の試験は直列化し、同じ種類は並行に走る。種類を宣言した thread の終了で保管場所は空に戻る。
- 最初は thread-local の上書きを試したが、dispatcher が blocking pool・worker thread で同梱の検査器を読むため、nextest でも 15 件以上落ちた。そのため採用しなかった。
- 宣言する箇所: `stub_checker::configure_passing_stub_checker`（Stub）、`dispatcher_without_bundled_checker` と `health_limits_review_unavailable_is_not_pass`（Unset）。`configure_bundled_checker` を直接呼ぶ試験は Custom（排他）になる。
- 旧い「process 共有の static 前提」の doc・コメントを直した（gate.rs・stub_checker.rs・dispatcher/tests/mod.rs）。

## 証拠（提出時の HEAD で実行）
- `cargo test -p task-dispatch health_review` → 134 passed、0 failed（exit 0）
- `cargo test -p task-dispatch` → 1760 passed、0 failed
- `cargo nextest run -p task-dispatch health_review` → 134 passed
- `cargo nextest run -p task-dispatch` → 1767 passed
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0

## 未解決・提案
- なし。`cargo clippy --workspace --all-targets` は task-core の lib test が既存の理由で落ちる（今回の変更とは無関係。未調査）。

## commit
- fix-cargo-test: 同梱の検査器を試験の種類ごとに直列化して cargo test でも health_review を通す
