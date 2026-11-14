# clippyfix: celeris の tests の既存 clippy 不備

状態: 一部完了（tests/ 配下は解消。src/ の lib test に 2 件残る）

## 変更
- `crates/celeris/tests/{campaign_lock_e2e,cap_probe_real_daemon,ops_r1_pipeline}.rs`: `#[path]` で同じ support_cases/mod.rs を読む `mod support_cases` に `clippy::duplicate_mod` の局所 allow を付けた（各 test は別 crate なので共通 module にできない。試験の挙動は不変）。
- `crates/celeris/tests/script_exhaustive.rs`: 未使用 import `ScriptGenerationControl` を削除。

## 証拠
- `cargo clippy -p celeris --test '*' -- -D warnings`: exit 0（提出時の HEAD で実行）
- `cargo fmt --all -- --check`: 差分なし

## 未解決
- `cargo clippy -p celeris --all-targets -- -D warnings` は `crates/celeris/src/delivery/health_evidence_tests.rs:73,172` の `clippy::type_complexity` で落ちる（lib test）。tests/ の外で、並行 WU `celeris` が src/delivery を触るため本 WU では直していない。

## 提案
- 該当 2 箇所の tuple 配列の型を `type` 別名に切り出す小修正を、`celeris` WU か統合時に行う。

## 追補（check 不合格の対応）
- 前回の check は `crates/celeris/src/delivery/health_evidence_tests.rs`（lib test）の `clippy::type_complexity` 2 件で落ちた。tests/ の外だが、受け入れ条件 0 を満たすため型 alias（`AppliesCase`・`EvidenceMutation`）を足して直した。試験の中身は変えていない。
- 提出時の HEAD で `cargo clippy -p celeris --all-targets -- -D warnings` と `cargo fmt --all -- --check` が exit 0。`cargo test -p celeris --lib health_evidence_delivery` は 8 件 pass。
