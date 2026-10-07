---
task: build-tmp-hygiene
wu: tmp-fix
status: done
completed: 2026-10-07
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
---

# tmp-fix: test-parallel.sh の後片付けが読み取り専用の試験残骸で失敗する

## 原因
cos_chat 添付の試験が dir 0o500 / file 0o400 を作り、`tempfile::TempDir` の Drop が消せず TMPDIR に `.tmp*` が残った。
test-parallel.sh の EXIT trap は `rm -rf` だけで、`set -e` 下で Permission denied → 非 0。
task-api `tests/changes.rs` の `env_with_gh` は `TempDir::keep()` で偽 gh の dir を意図せず残していた。

## 直したもの
- `scripts/dev/test-parallel.sh`: trap を `chmod -R u+w` → `rm -rf` にし、失敗は警告のみ（exit code 不変）。
- `crates/task-dispatch/src/test_support.rs`: `WritableTempDir`（Drop で権限を戻してから消す guard）。本番の 0o500/0o400 は不変。
- 使用側（cos_chat tests / harness_tests / sink_tests / tests/cos_chat_{attach_handoff,launch,rollover,triage}.rs / cos_live_fix_d4.rs）を差し替え。
- `crates/task-api/tests/changes.rs`: `keep()` をやめ TempDir を返す。

## 証拠
- `sh scripts/dev/check-test-tmp-leftovers.sh -p task-dispatch|task-api|celeris` → 3 つとも `no leftovers`（修正前は task-dispatch 7 件・task-api 8 件）。
- `CELERIS_TEST_SKIP_DOC=1 bash scripts/dev/test-parallel.sh` → exit 0、passed 4661 / failed 0、`tmp_leftovers: 0`、`test-parallel: ok`。
- `cargo clippy --workspace -- -D warnings` → 通過。

## 未解決
`cargo clippy --workspace --all-targets` は既存の 2 件（celeris/tests/model_role_assignments_consistency.rs の cloned_ref_to_slice_refs、task-dispatch undeclared_artifacts/tests.rs の useless_vec）で落ちる。範囲外で未修正。
