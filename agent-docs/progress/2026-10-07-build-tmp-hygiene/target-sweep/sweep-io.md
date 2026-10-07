---
title: 葉 sweep-io — target_sweep の I/O 層（task-dispatch）
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 sweep-io

ADR 2026-10-07-build-tmp-hygiene D1.2 規則 4〜6・D1.3 の I/O 層を `crates/task-dispatch/src/target_sweep.rs`（試験 `target_sweep/tests.rs`）に置いた。

- 公開 API: `run_sweep(roots, params, mode: TargetSweepMode, env: &dyn SweepEnv) -> SweepReport`。
  `SweepEnv` は `now()`・`bytes()`（既定 `st_blocks × 512`）・`used_at()` を差し替えられる trait（`SystemEnv` が本物）。
- 走査: root の直下と `<root>/<repo-key>/wu-*` を target dir、`.cargo-lock` を持つ `<profile>`・`<triple>/<profile>` を profile dir。
  `deps`・`.fingerprint`・`build`・`incremental` の直下だけを項目にする。symlink は辿らない（項目なら `symlink` で skip）。
  `.deleting-*`（profile 内・root 直下・repo-key 直下）は前回の残りとして集める。
- lock: profile ごとに `.cargo-lock` を読み取りで開いて `Flock::lock(LockExclusiveNonblock)`。取れなければ `locked` に入れて
  `task_worker::target_sweep::plan` へ（項目は 1 つも消えず `build_in_progress`）。apply は rename まで lock を持ち、dry_run は確かめてすぐ放す。
- apply: 放置 target dir を `<親>/.deleting-<ulid>` へ先に rename、残りの項目を `<profile>/.deleting-<ulid>/<相対 path>` へ rename →
  lock を放す → 新旧の `.deleting-*` を `remove_dir_all`。rename の失敗は `rename_failed` で skip に積み、量に数えない。
- `SweepReport`（Serialize/Deserialize）: mode・root ごとの before/after/deleted bytes・items・by_reason・deleted・skipped・
  over_cap_unresolved・leftovers・errors・duration_ms。`to_event()` で `Event::TargetSweepRan`（skipped は先頭 50 件＋総数）、
  `root_reports()`・`skipped_head()` で task_core の型へ写す。
- root の列挙は呼び出し側。scratch pool の `release-build` lease は roots に入れない限り触らない。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-dispatch --lib target_sweep` | exit 0、6 passed（`target_sweep_skips_profile_with_held_cargo_lock`・`target_sweep_dry_run_deletes_nothing`・`target_sweep_apply_removes_old_items_and_reports_bytes`・`target_sweep_apply_reports_real_block_bytes`・`target_sweep_never_touches_live_release_lease`・`target_sweep_cleans_leftover_deleting_dirs`） |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし |
| `bash scripts/dev/test-parallel.sh` | exit 0、4677 tests run: 4677 passed, 13 skipped |

試験は `tempfile::TempDir` の偽の target 木と実 flock（試験が `.cargo-lock` を `Flock::lock` で握る。cargo は走らせない）で決定的。
時刻は `File::set_times` で置き、時計は固定。sleep・負荷なし。

## 未解決事項

- 走査と lock の間に cargo が項目を使って終わった場合、その項目は走査時の古い使用時刻のまま消えうる（lock 下で再 stat はしない。
  cargo が作り直すだけで壊れはしない）。
- `remove_dir_all` は `run_sweep` の中で lock を放した後に同期で行う（別スレッドにはしていない）。daemon から呼ぶときは
  `spawn_blocking` 等で tick の外に出す（sweep-cron 葉）。
- `cargo clippy --workspace --all-targets -- -D warnings` は既存の 2 件（`crates/celeris/tests/model_role_assignments_consistency.rs:388`、
  `crates/task-dispatch/src/undeclared_artifacts/tests.rs:146`）で落ちる。この葉の変更ではない。
- 2026-10-07 attempt 2: 計画の check `cargo clippy -p task-dispatch --all-targets -- -D warnings` は base（a416f89b）から変わっていない
  `crates/task-dispatch/src/undeclared_artifacts/tests.rs:146` の `clippy::useless_vec` で落ちる（exit 101）。この葉の範囲 check はこのファイルを
  許可しないので、この葉では直せない。`-A clippy::useless_vec` を付けると exit 0（この葉の変更は警告なし）。`target_sweep_` 試験 6 本は pass。
  plan_issue として申告した（範囲に当該ファイルを足すか、check から除くか、別の修正葉を先に置く）。
- 2026-10-07 attempt 3（replan 後）: 範囲に許可されたので `undeclared_artifacts/tests.rs:146` の `vec![...]` を配列にした（挙動不変）。
  `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0、`cargo test -p task-dispatch --lib target_sweep_` → 6 passed（exit 0）、
  `cargo test -p task-dispatch --lib undeclared_artifacts` → 18 passed。

## 提案

- ADR D1.1 の「使用時刻 = file・dir の max(mtime, atime)」は、dir については mtime だけにした。掃除の走査自身の `read_dir` が
  relatime で dir の atime を進めるため、atime を数えると dir の項目（`.fingerprint/…`・`build/…`）が永久に古くならない。
  ADR の付記にこの差を書くことを提案する（close 葉）。
