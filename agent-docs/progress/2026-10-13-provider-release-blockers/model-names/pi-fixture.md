# model-names / pi-fixture

- 対象: `crates/celerisctl/src/commands/worker_tests.rs` の `route_config()` の `[harnesses.pi]`
- 変更: `kind = "pi"` の下に `extensions = ["extensions/hashline.ts"]` と `tools = ["hashline_read", "hashline_edit", "bash"]` を追加（`crates/celeris/src/config/tests.rs` の例と同じ値）
- 原因: task branch の基点（wu/pi-harness 370464b6）で `Config::load` が Pi に非空の extensions と tools を要求するようになった（panic: `[harnesses.pi] Pi requires non-empty extensions and tools`）
- 範囲: `kind = "pi"` を持つ試験 fixture は workspace に 2 箇所。`config/tests.rs` は両欄が既にあり変更不要。`worker_tests.rs` だけ直した。本体コードは変えていない。

## 検証

- `cargo test -p celerisctl worker_route_cli` → 5 passed, 0 failed（account_flag_narrows_the_pool / fails_closed / openai_compatible_source_carries_its_endpoint / model_flag_selects_that_model / uses_the_given_harness_and_the_first_declared_candidate）
