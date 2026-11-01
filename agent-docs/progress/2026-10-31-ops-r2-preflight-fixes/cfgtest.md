---
status: done
---
# cfgtest: ops_r1_measure 試験を --policy-version 必須に合わせる

- `crates/celeris/tests/ops_r1_measure.rs` の config script 呼び出しに `--policy-version 6` を足した。
- release の cos-inbox-triage skill は版 10 で、`--policy-version 6` と食い違い script が止まる。試験は `config/skills` の写しを作り、版だけ 6 に直して `--skills-src` に渡す。期待値 "6"（health・operation）は変えていない。script 側の必須化は戻していない。
- 旧引数（`--claude-accounts` 等）は試験に無い。
- 証拠: `cargo test -p celeris --test ops_r1_measure` → exit 0、1 passed。
- test-affected・clippy は統合段で実行する。`cargo fmt --all` 適用済み。
