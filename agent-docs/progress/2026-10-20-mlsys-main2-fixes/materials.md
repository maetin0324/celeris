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
- 凍結済みの `agent-docs/progress/2026-10-16-mlsys-final/final-input-manifest.json` は旧 prompt（`公開 case 定義は crates/campaign/cases/final/<case>/case.json`）のままで、書き換えない（過去の freeze の証跡）。そのまま次の pilot に使うと実行前検査（`check_prompt_reference`）が拒否する。
- 次の campaign には、prompt を `materials/<case>/case.json` 参照に直した新しい input manifest を作り（problem の `sha256` と manifest の `sha256` を `manifest_hash` で作り直す）、それを `input.path`/`input.sha256` に持つ新しい campaign manifest を新 campaign id で作って再 freeze する（`validate`・`dry-run`・`offline-lock`・preflight をやり直し、campaign sha256 の承認も取り直す）。旧 campaign id の manifest・入力は変えない。
- 試験は凍結済みの manifest の写しを組み立て時に直して使う（`crates/campaign/tests/preflight_setup/mod.rs` の `write_materials_prompt_manifest`）。実行用の新 manifest はこれとは別に作る。

## build2 統合後の修正
- `arm_set_cli_normal` の fixture の prompt と、`launcher`・`trial_state_cli_e2e` が使う `preflight_setup` の manifest 写しの prompt を、`materials/<case>/case.json` 参照に直した（検査は緩めていない）。
