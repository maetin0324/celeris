# ADR 2026-10-29: SQLite の memory 統計を process の load 時に切る

- 状態: 実装済み（2026-10-29）
- tasks: [01M6320TM5XCF2QA8XXTT57BRE]
- 関係: ADR-0064（SQLite store の接続と PRAGMA）、ADR 2026-10-20-full-tests-at-release-only

## 問題

`cargo test --workspace` が 1772 秒かかり、受け入れの check（上限 1200 秒）で打ち切られた。nextest（1 試験 1 process）では
同じ試験が 130 秒で終わる。`task-dispatch` の unit 試験（1 process・24 thread）は 375 秒のうち kernel 時間が 6563 秒で、
strace では futex が syscall 時間の 90%。待っていたのは bundled SQLite の `sqlite3Malloc` が取る process で 1 つの
mutex（memory 統計 `SQLITE_CONFIG_MEMSTATUS`、既定で有効）だった。同じ process で多数の接続を並行に使うと、確保のたびに競合する。

## 決定

- D1: `task-core` の `sqlite_process` module が `.init_array` に登録した関数で、load 時（main・試験 thread の前）に
  `sqlite3_config(SQLITE_CONFIG_MEMSTATUS, 0)` を 1 回呼ぶ。`sqlite3_config` は初期化前にしか効かず、他の SQLite 呼び出しと
  並行に呼べないため、thread が 1 本の load 時に置く。
- D2: rlib の object が link から落ちると登録も消えるので、`SqliteStore::from_connection` から `keep_linked()` で参照する。
  `task-core` を link する binary（daemon・API・試験）はすべて対象になる。
- D3: この repo は `sqlite3_memory_used`・soft heap limit を使わない。統計を切っても挙動は変わらない。
- 不採用: `LIBSQLITE3_FLAGS=-DSQLITE_DEFAULT_MEMSTATUS=0` を `.cargo/config.toml` に置く案。この file は git の管理外で、
  `scripts/dev/worktree-target-dir.sh` が生成・削除し、評価器の作業場所には無い。

## 結果

`task-core` の unit 試験 148 秒 → 12 秒、`task-dispatch` 375 秒 → 37 秒、`cargo test --workspace` 1772 秒 → 829 秒
（同じ host、負荷あり）。回帰の試験 `memstatus_is_off_before_the_first_connection` は確保後も `sqlite3_memory_used()` が 0 であることを見る。
