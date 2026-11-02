---
title: S2 deploy-cmd — bundle を state_dir に配備する script-deploy command
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 deploy-cmd

## 結論
- S1 の未解決 2（bundle を配備する command が無い）を埋めた。`crates/script-runtime/src/bin/script-deploy.rs` を足した。
  - 使い方: `cargo run -p celeris-script --bin script-deploy -- --dist <dir> --state-dir <dir>`
  - 既存の `manifest::deploy_bundle` を呼び、`<state_dir>/script-bundles/<bundle_sha256>/` に配備する。成功すると `bundle_sha256` を stdout に 1 行だけ出し、exit 0。
  - 同じ hash の再配備は既存を検証して再利用する（来歴 `toolchain`・`source_commit` だけの違いは既存の manifest を残す）。
  - 契約（`host_api`・`schema_sha256`・`domains`・`capabilities`・`checkpoint_versions`）の食い違い、hash の不一致、引数の誤りは stderr に理由を出して非 0。stdout は空。境界 ADR の「TS の置き場と配備先」のとおりで、ADR は変えていない。
- 試験は `crates/script-runtime/tests/script_deploy.rs` の 3 件。全て tempdir の state dir に `tests/fixtures/selftest-bundle/` を配備する。本番の state_dir には書かない。
  - `script_deploy_new_prints_sha_and_writes_bundle`（新規）
  - `script_deploy_same_hash_reuses_existing`（再利用）
  - `script_deploy_rejects_manifest_conflict_and_invalid_input`（拒否: 契約の食い違い・hash 不一致・引数不足）

## commit（件名）
1. `phase s2: deploy-cmd: bundle を state_dir に配備する script-deploy command と試験`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-script -E 'test(script_deploy)'` — exit 0、3 passed。
- `bash scripts/dev/test-affected.sh` — exit 0。対象 crate は celeris・celeris-script・celerisctl、884 passed、0 skipped。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。

## 未解決事項
- 本番の `[script] state_dir` への配備手順（`control-ts/dist` を作ってから `script-deploy` を走らせ、出た hash を `active_bundle_sha256` に入れる）は close の人の手順で書く。

## 提案
- なし。
