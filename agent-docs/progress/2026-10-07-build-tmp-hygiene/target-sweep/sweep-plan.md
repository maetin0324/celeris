---
title: 葉 sweep-plan — target_sweep の純粋な計画関数（task-worker）
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 sweep-plan

ADR 2026-10-07-build-tmp-hygiene D1.1〜D1.3 の純粋部分を `crates/task-worker/src/target_sweep.rs`（試験 `target_sweep/tests.rs`）に置いた。

- 型: `TargetSnapshot`（root・target_dir・profile_dir・path・kind `deps|fingerprint|build|incremental`・key・bytes・used_at・symlink）、
  `SweepParams`（Default: 7 日・120 GiB・0.8・14 日、roots は空＝呼び出し側が渡す）、
  `SweepPlan`（`delete{root,path,bytes,reason age|cap|stale_target}`、`skip{path,reason build_in_progress|outside_root|symlink}`、
  root ごとの `before_bytes/after_bytes/over_cap_unresolved`、全体の `over_cap_unresolved`）。すべて `serde::Serialize`（snake_case）。
- `plan(snapshot, &locked, now, &params)` は I/O なし。規則の順: 検証（root 外・`..`・profile 外は `outside_root`、symlink は `symlink`）
  → lock の取れない profile は 1 件の `build_in_progress` にまとめ項目を 1 つも消さない → 古さ → 上限（古い順・同時刻は path 辞書順・上限×ratio 以下まで）
  → 放置 target（lock・skip を含む target は対象外。先の段で計画済みの分は bytes に数えない）。
- deps・.fingerprint・build は (profile, key) で 1 単位（使用時刻は単位の最大）。incremental は別単位。
- key: `deps_key(file 名)`（lib 成果物だけ先頭 `lib` を落とす。hash が 16 進でなければ None）、`dir_key`、`group_deps_by_key`。

## 証拠

- `cargo test -p task-worker --lib target_sweep_` → exit 0、`test result: ok. 9 passed; 0 failed`
  （deletes_items_older_than_max_age・trims_oldest_until_80pct_of_cap・cap_is_not_applied_under_cap・skips_locked_profile_in_plan・
  groups_deps_by_key・stale_target_dir_whole・rejects_outside_root_and_symlink・plan_serializes_to_json・params_default_values）
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 4670 / failed 0 / ignored 14

## 未解決事項

- `cargo clippy --workspace --all-targets -- -D warnings` は範囲外の既存試験 2 件で落ちる
  （`crates/celeris/tests/model_role_assignments_consistency.rs:388` cloned_ref_to_slice_refs、
  `crates/task-dispatch/src/undeclared_artifacts/tests.rs:146` useless_vec）。この葉では触っていない。

## 提案

- root の `before_bytes` は snapshot の項目の合計（profile 直下の最終 binary・doc 等は入らない）。I/O 層（sweep-io）で
  root 全体の実使用量を上限判定に使いたければ、`plan` に root ごとの「項目以外の量」を渡す欄を足す。
