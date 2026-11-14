---
title: 工程 consumers の統合後の検査の repair（cargo test -p task-dispatch の 1 件）
tasks: [01M7CFRYZH0AW00M3WAHXX8B57]
status: done
updated: 2026-11-14
---
# 工程 consumers の統合後の検査の repair（cargo test -p task-dispatch の 1 件）

完了日: 2026-11-14

基準は `$CELERIS_WU_BASE`。統合後の `cargo test -p task-dispatch` で
`health_enforce_g2_unavailable_is_not_pass` が 1 件落ちた（`assert_waits` の承認の子が 0 件）。単独で走らせると通る。

## 原因
`health_review/policy_review/unknown_tests.rs` の `case_for` は、材料作り（`build_health_input`）を
同梱の検査器の宣言（`dispatcher_without_bundled_checker` の中の `bundled_test::require(Kind::Unset)`）より前に呼んでいた。
`build_health_input` は `gate::bundled_checker()` を読む。`cargo test` は 1 process で試験を並行に走らせるので、
宣言前のこの間に `Kind::Stub` の試験が合格の stub を設定していると、検査器の無い repo の材料が stub で作られる。
その場合は `checker_missing` が記録されず、承認の子が作られない。今回の変更（repo 単位の auto_integrate）とは関係がなく、
並行の順序で起きる試験の競合である。

## 修正
`case_for` で `dispatcher_without_bundled_checker` を `build_health_input` の前に移した（宣言の後に材料を作る）。
本体のコード・試験の断言は変えていない。`bundled_test` は宣言のある間だけ種類の違う試験を待たせるので、
宣言の後に材料を作れば stub の試験と重ならない。

## 証拠（提出時の HEAD で実行）
- `cargo test -p task-dispatch` → exit 0、1762 passed / 0 failed（lib）、7 passed（他）
- `cargo test -p task-dispatch --lib -- health_enforce_g2_unavailable_is_not_pass`（修正前、単独）→ 1 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲外差分の検査 3 件: `crates/task-dispatch/` の検査は exit 0。`crates/celeris/tests/…` と `crates/celeris/` の検査は
  `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs` を範囲外として落ちる。3 件は統合した WU
  （clippyfix・celeris・dispatch）それぞれの範囲で、互いに排他なので、コードを 1 file でも変えると最低 2 件は落ちる。

## 未解決事項
- 範囲外差分の検査 3 件は、この repair の範囲（`crates/celeris/` と `crates/task-dispatch/` の和）を表していない。

## 提案
- 統合後の repair には、統合した WU の範囲外検査を 1 件ずつ当てず、範囲の和で 1 件の検査を当てる。
