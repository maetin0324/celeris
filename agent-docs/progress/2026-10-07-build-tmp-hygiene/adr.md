---
title: 葉 adr — ADR 2026-10-07-build-tmp-hygiene を書く
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 adr

- 読んだもの: `scripts/dev/worktree-target-dir.sh`、`crates/task-worker/src/build_cache.rs`、`scripts/selfdeploy/release.sh`
  （release-build の scratch lease・`.lock-release`）、`crates/task-dispatch/src/scratch_gc.rs`・`dispatcher/housekeeping.rs`、
  `crates/celeris/src/config/cron.rs`・ADR-0131、ADR-0074 の `min_free_disk_mb`、ADR-0133（NoticeKind 9 種・InboxKind 14 種）、
  `scripts/dev/test-parallel.sh`、`crates/task-worker/src/browser_tests.rs`、`web/e2e/**` の mkdtemp。
- 決定（ADR §2）:
  - D1: roots 既定 `<build_cache_dir>/cargo` と `/var/tmp/agent-platform-build`。項目は deps・.fingerprint・build・incremental。
    7 日より古い項目を消し、120 GiB/root を超えたら古い順に 80% まで。`.cargo-lock` を非 blocking flock で取れない profile は
    skip（build_in_progress）。release-build は release.sh が `.lock-release` を持ったまま自分で sweep。放置 target は 14 日で丸ごと。
    純粋 `plan` と I/O 層を分離。cron（`extra.action = "target_sweep"`、決定的 executor、seed は無効）と
    `celerisctl target sweep --dry-run|--apply`。event `TargetSweepRan`＋通知 `cron_run`。
  - D2: `<workspace>/<task>/runs/<run_id>/tmp` に TMPDIR/TMP/TEMP、終端すべてで削除、前置きの固定文。
  - D3: tempfile / guard、web e2e の共通 helper `web/e2e/support/tmp.ts`、test-parallel.sh は `TMPDIR=$logdir/tmp`、
    証跡は `CELERIS_TEST_ARTIFACTS_DIR`。
  - D4: `/`・`/local`・`/tmp`、80% で通知 `disk`、95% で受信箱 `disk_full`、level 上昇時のみ・24h 再通知・5 ポイントのヒステリシス、
    状態は表 `disk_watch_state`。
  - D5: 試験接頭辞 `target_sweep_` / `run_tmpdir_` / `disk_watch_`。
- 証拠: `git diff --name-only "$CELERIS_WU_BASE"` が agent-docs/ の 3 file だけ（コード変更なし）。

## 提案

- ADR-0131 に「`extra.action` の雛形は worker に渡さず決定的 executor が処理する」付記を target-sweep 葉で書く。
