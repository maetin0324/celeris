---
title: "stranded sweep を single-flight にし、取り込み済みで head 不変の記録は git を呼ばない"
tasks: [main-direct]
status: done
updated: 2026-10-12
completed: 2026-10-12
---

# stranded sweep single-flight（2026-10-12、本番 hotfix）

- branch: `hotfix/stranded-sweep-single-flight`（main 3e80225f から）。決定は [ADR 2026-10-12-stranded-sweep-single-flight](../adr/2026-10-12-stranded-sweep-single-flight.md)。
- merge・deploy・push はしていない（operator が行う）。

## 障害

- `stranded_sweep::spawn` が 300 秒ごとに無条件で `spawn_blocking(run)` を起こし、NFS 上では 1 回が 300 秒を超えて sweep が重なった。
- `git merge-base --is-ancestor <sha> refs/heads/main` が 44 本以上 D state、tick 最大 7014 秒、dispatch が数時間停止、task 詳細 API が応答しない。
- 定常状態でも done root task（約 1000 件）ごとに `rev-parse` と `merge-base` を呼んでいた（integrated で head 不変の記録も）。

## 直したこと

- `crates/celeris/src/daemon/stranded_sweep.rs`
  - `SingleFlight` / `FlightGuard`: 前回の sweep が走っている間の tick は warn（`running_secs`）を出して何もしない。guard の drop で解放（エラー・panic でも）。
  - `RepoGitCache`: sweep 1 回につき repository ごとに `git for-each-ref refs/heads/celeris/` 1 回と default branch の解決 1 回。task ごとの `rev-parse --verify` をやめた。`task.repos` 無しの task（`repoless_records`）も同じ表を使う。
- `crates/task-ops/src/stranded.rs`
  - `sweep`: 保存済みの行が integrated・理由なしで branch と head が同じなら `merge-base` を呼ばずに飛ばす（`unchanged_integrated`）。解決に失敗する記録（remote・path 無し・default branch 不明・head 無し）は従来どおり判定する。保存済み行の読み出しは 1 記録 1 回にした。
  - `promotion_view`: `(path, head, release_sha)` → 祖先か を process 内に覚える（git が exit 0/1 で答えた場合だけ。上限 4096 で全消去）。

## 試験

- `daemon::stranded_sweep::tests::stranded_sweep_overlapping_ticks_do_not_start_a_second_run`（paused time、blocking 側は channel で止める。3 tick 重ねても開始 1 回、解放後の tick で 2 回目）
- `stranded_sweep_slot_is_released_after_a_panicking_sweep`
- `stranded_heads_are_read_once_per_repository`
- `stranded::tests::stranded_sweep_skips_git_for_unchanged_integrated_record`（存在しない repo path。git を呼べば unintegrated になるが、行・checked_at・events は変わらない。head が動くと再判定される）
- `stranded::tests::stranded_promotion_remembers_decided_ancestry`（repo を消しても覚えた答えを返す。git が失敗した答えは覚えない）

## 証拠

- `cargo test -p task-ops stranded` → exit 0、19 passed（unit）。
- `cargo test -p celeris stranded` → exit 0、unit 4 passed、`integration_api_stranded_branch_round_trip_through_routes_and_events` 1、default branch 系 3、`stranded_detects_inbox_merge_discard_hold_and_promotion_independently` 1 passed。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-parallel.sh`（TMPDIR=/var/tmp）→ exit 100、5214 run / 5212 passed / 2 failed / 13 skipped。
  失敗は `celeris releases::tests::promoting_via_fake_systemd_run_writes_the_lock_and_streams_the_log`（tests.rs:924）と
  `releases::tests::promotion_runs_the_promote_script_of_the_current_release`（tests.rs:386）で、どちらも今回触っていない releases の
  promote 待ち（5 s / 9 s で期限切れ）。単独の `cargo test -p celeris --lib releases::tests::promot` → 11 passed / 0 failed。
  全体負荷下のタイミング依存と判断（今回の変更とは無関係）。

## 本番への移行

- config の変更なし。daemon を差し替えるだけ。

## 未解決事項

- 1 回目の sweep（起動直後）はこれまでどおり全記録を判定する。integrated の行が揃えば 2 回目以降は repository ごとの数回の git になる。
- main を巻き戻す運用をした場合、巻き戻し前に integrated になった記録は head が変わるまで integrated のまま（ADR D2）。

## 提案

- releases の promote 試験 2 件が全体実行の負荷下で落ちる。出来事待ちに直す（testing guide）。

- sweep の所要時間を metrics / ログに出し、INTERVAL に近づいたら気付けるようにする。
