---
unit: materials
status: done
---
# materials: 公開 case 定義を trial workspace に置く（ADR 2026-10-20-mlsys-main2-fixes D2-4）

## 実装
- `Case::public_json`（private_ref・grade_case_ref を除く）、`Seeder::make` が `materials/<case>/case.json` を各試行 workspace に配る。
- 配った file は `inputs::artifact_hash` で「seed から変わったか」の判定から除く（runner の no_artifact・checker）。
- `check_prompt_reference`: harness/celeris の実行前に、prompt が `materials/<case>/case.json` を参照し repo 内 `cases/final/` を参照しないことを検査。

## 証拠
- `cargo test -p celeris-campaign --test public_case_materials` → 3 passed
- 変更に触れる 11 試験 binary + lib: 全て ok（exit 0）
- `cargo fmt --all -- --check` exit 0 / `cargo clippy -p celeris-campaign --all-targets -- -D warnings` exit 0
- `tests/ablation_application.rs` の daemon 試験は `unknown field max_tokens`（task-api 側の欄は budget unit の変更で、この worktree には未統合）。integrate-build2 後に再確認が要る。
- 補足: host の sccache が固まり build が止まったため `SCCACHE_BIN=/nonexistent`（wrapper の公式の迂回）で実行した。

## 本番への移行
- 次の campaign の input manifest の problem prompt を `materials/<case>/case.json` 参照に直す（旧 `crates/campaign/cases/final/<case>/case.json` 参照は実行前検査で拒否される）。
- 実行時の `--materials-src` は `<case>/` ディレクトリを含むこと（無いと配送が fail-closed）。
