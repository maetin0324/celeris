---
task: 01M5ANK5TMYWWH537DPGKYSE4S
wu: seed-path-fix
status: done
completed: 2026-10-19
---

# seed-path-fix: seed の絶対 path 焼き込みを壊さない

## 原因（2 層）
1. `refresh_seed` が `.building-gen-*/target` で build して `gen-*` へ rename → cargo が焼いた `CARGO_TARGET_DIR` / dep-info の path が消える（celerisctl 統合試験 46 件 NotFound）。
2. （operator 報告）build 先を直しても seed の `checkout/` の path（`CARGO_MANIFEST_DIR`）が試験 binary に残り、owner の試験が task の worktree でなく seed（main）の file を読む → 誤った合格。

## 変更（crates/task-worker/src/scratch のみ）
- `seed.rs`: 最終 path `gen-*/target` で build。manifest（一時 file → rename）の出現が完了印。manifest の無い `gen-*`（current を除く）は refresh 冒頭・`seed_gc` が掃除（`abandoned_builds`）。`.building-*`・`SEED_BUILDING_PREFIX`・`gen-*` への rename 経路を削除。
- `reflink.rs`: `prune_workspace_artifacts`。seed から写した後、workspace member（dep-info が seed の checkout 配下の絶対 path または cwd 相対 path を指す unit）の `deps/<unit>.*`・`.fingerprint/*-<hash>`・`build/<pkg>-*`、`incremental/`・`examples/`・profile 直下の bin を消す。registry/git の依存の成果物だけ owner へ残る。
- `tests.rs`: 回帰試験 `seed_is_built_at_its_final_path`（修正前に失敗を確認: `.building-gen-…/target` ≠ `gen-…/target`）、実 cargo の `seed_copy_drops_workspace_member_artifacts_and_keeps_registry_ones`（seed binary は checkout の path を持つ／owner へ写した後は member 成果物ゼロ／owner worktree で build し直すと binary は owner の path を持ち seed の path は 0 件／registry 相当は残る）。GC 試験は未完 `gen-*` 前提へ更新。
- 旧 progress 文書 `2026-10-02-host-sccache-reflink/seed-refresh.md` の build path の記述を更新。

## 本番への移行
- 人が実行: release 後に `seed_reflink = true` を戻す。`seeds.broken-20261019` は人が確認して消す。旧形式の `.building-*` が seed dir に残っていれば手で消す（コードは掃除しない）。

## 証拠コマンド
- `cargo test -p task-worker --lib scratch::` → 39 passed
- `bash scripts/dev/test-parallel.sh` → 5836 passed, 0 failed（exit 0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
