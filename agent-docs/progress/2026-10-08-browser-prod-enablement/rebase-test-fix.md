# Browser policy auto-attach rebase test fix

tasks: [01M4CTQ1B4FX85F28A74VPQGTN]

## 原因と修正

main rebase 後、`BrowserCapability` に追加された `approval_actions` を `crates/task-ops/tests/browser_policy_autoattach.rs` の `BrowserCapability` struct literal が指定しておらず、nextest の compile 段階で E0063 になっていた。試験の意図は承認 action の追加設定が無い grant なので、literal に `approval_actions: vec![]` を追加した。試験期待値と本体コードは変更していない。

## 検証

- `cargo check --workspace --tests` — exit 0。
- `bash scripts/dev/test-parallel.sh` — exit 0。nextest 4,789 passed / 0 failed（13 skipped）、doc tests 成功（1 ignored）。まとめ: `CELERIS_TEST_SUMMARY` の `ignored` は nextest 13 と doctest 1 の合計 14。
- `cargo clippy --workspace -- -D warnings` — exit 0。
