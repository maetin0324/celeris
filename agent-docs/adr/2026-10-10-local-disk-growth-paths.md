# ADR 2026-10-10: `/local` を消費する 4 経路の原因と対策

---
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV]
---

- 日付: 2026-10-10
- 状態: 実装済み（2026-10-10）
- 本番手順: [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md)（配送後に人が実行する差し替え・刈り込み・確認）
- 関連: [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)、[ADR-0075](0129-host-sccache-reflink-targets.md)、[ADR-0129](0136-local-hot-data-layout.md)、[ADR-0136](0136-local-hot-data-layout.md)

## 背景

`/local` の容量を継続的に消費する経路を、repo 作業用 target、release-build の scratch lease、DB backup、release target symlink の 4 系統に分けて調査した。調査はコードと既存 findings の読み取りで行い、本番ファイルや DB の変更・削除はしていない。

### 1. repo 直下 `target/` が残る

`crates/task-worker/src/workspace_targets.rs` の `task_targets`（139–162 行）は task root とその `wu/` 内にある `repos/`・旧 `tree` worktree だけを列挙する。`finished_task_targets`（173–211 行）は Done/Cancelled/Failed で、終端時刻から猶予を過ぎ、running run が無いものだけを候補にする。既定猶予は `crates/task-dispatch/src/target_sweep.rs` の `DEFAULT_WORKSPACE_TARGET_AFTER_SECS`（651 行、6 時間）と `crates/celeris/src/config/maintenance.rs`（119–143 行）。実削除には maintenance の `target_sweep` が Ready で apply として実行される必要がある（`crates/task-dispatch/src/dispatcher/maintenance.rs:119–143,162–203`）。cron の seed は無効で、DB に cron job が無ければ投入もされない（`crates/celeris/src/config/cron.rs:1–5`、試験 `cron_tests.rs:320–339`）。

木の子 task が親 checkout を再利用すると、target は子 task root の外にある。task ごとの `task_targets` は親の repo checkout を走査せず、親 checkout の利用者である子の終了後も候補にしない（`workspace_targets.rs:141–162,202`）。子の終端だけを理由に親 checkout を消すのも誤りなので、owner と参照関係の追跡が要る。

task `01M4D7RVKX` は 2026-10-09 08:01 に終端したものとして調査した。workspace root の読み取り専用探索では該当 target が見つからず、live DB と sweep event も確認できないため、個別の残存理由は断定できない。コード上は、子が親 checkout を使ったため task root の掃除対象外だった可能性、6 時間猶予（同日14:01まで）、または apply cron が未有効・未実行だった可能性がある。単に終端から時間が経っただけでは削除されない。

### 2. release-build lease の test binary が増える

`scripts/selfdeploy/release.sh:180–186` は同じ build tree の場合 `.celeris-release-tree` marker により workspace crate clean を省き、固定 target を再利用する。Cargo は依存・feature・rustc 等で hash が変わった古い成果物を消さない。梱包後の sweep（同 `:563–566`）は target directory 自身を `--root` に渡す一方、`crates/task-dispatch/src/target_sweep.rs:350–397` は root の子を target root と解釈し、その子の下に profile lock を探す（同 `:203–224`）。この階層ずれで対象が 0 件になる。さらに外部 sweep は `release-build` owner を除外し（同 `:295,306`）、scratch GC は P0 を選ばず（`crates/task-worker/src/scratch/gc.rs:337`）、48時間 TTL 内に touch される release-build lease には上限が効かない。本番 lease は 128.6 GB、その `debug/deps` には workspace crate test binary 355 本・約99.9 GiB が観測された。

直前 release の**build 開始時刻**は保存されていない。`release.sh:93` の `RELEASE_T0` は一時値で、gate JSON の `at` と manifest `built_at` は gate 終了後（`:408,694`）。開始時刻を記録し、workspace crate の test binary と対応 `.d` のうち開始時刻より古いもの、live 参照のない hash 世代を刈る必要がある。依存 crate の rlib は共有依存成果物なので保持する。上限を超えた場合は対象 lease を作り直す。現状本番に seed がないため再作成は空 target からとなり、release gate を約1時間再実行するコストがある。

### 3. DB backup の無制限・偏った保持

`scripts/selfdeploy/promote.sh:179–183` は `<TS>-pre-<sha12>.sqlite3` を作るが、promote/rollback/lib に prune がなく、`db_maintenance::prune_backups` は `celeris-*.sqlite3` のみを見る（`crates/celeris/src/db_maintenance.rs:273–295`）。本番では promote backup 51本、約21 GiB。

定期 backup は `db_maintenance.rs:177–225,273–289` が `VACUUM INTO` 後に新しい順で `backup_keep` 本だけ残す。本番設定は1時間間隔・48本（`config.toml:416–422`）、約28 GiB。日次/週次の復元点、合計 byte 上限、削除前の integrity check はない。2種の backup は同じ `/local/celeris/state/backups` にある。

### 4. `.cargo-target` symlink が dangling になる

`scripts/selfdeploy/lib.sh:43–50` は `$SD_RELEASES/.cargo-target` を fallback とする。`sd_scratch_lease`（同 `:653–700`）が成功すれば `SD_CARGO_TARGET` は lease path に変わる。lease GC が実体を消しても symlink を unlink・再解決する箇所がないため、fallback symlink が dangling になり得る。`scripts/selfdeploy/browser-ledger.sh:78–79` は `sd_scratch_lease` を呼ばず caller env または `$SD_CARGO_TARGET` を使うため、dangling fallback を選び得る。

## 決定

### D1. 全 Rust 実行経路に `CARGO_TARGET_DIR` を渡す

ローカル repository を使う worker run、WU check、`repos=[]` の子、integration check、reviewer run と reviewer checks に、所有 task/WU の target lease を明示して渡す。worktree が空・legacy・remote/non-git の場合にも cwd の repo checkout を解決できる範囲で target を指定し、解決不能なら明示的に理由を記録して repo 直下 target を防ぐ。adapter の `with_env` 非対応も失敗または警告だけで黙って fallback させない。既存経路は `crates/task-dispatch/src/dispatcher/{worker_task.rs,work_units.rs,phase_integration.rs,review_spawn.rs,housekeeping.rs,workspaces.rs}` と `crates/task-worker/src/build_cache.rs` にある。

**付記（2026-10-10、worker-target-env 実装）**: `crates/task-dispatch/src/dispatcher/workspaces.rs` の `CargoTargetFallback` と `Dispatcher::cargo_target_repo` / `Dispatcher::cargo_target_fallback` が、先頭が git でない作業場所の run・検査の checkout を決める（task の Local path・`kind = dir` repo が Rust ならそれ、無ければ祖先〈上限 8〉の作業ツリーがある Rust の git checkout。owner は自分の `task-<id>`）。worker run は `RunExtras.cargo_target_fallback` 経由で `run_worker` が、WU check・統合の検査・reviewer は `check_cargo_target_env` / `with_review_cargo_env` が使う。渡せないとき（祖先の checkout 未作成・adapter の `with_env` 非対応）は warn と run の `WorkerProgress`（status、接頭辞 `cargo-target:` = `CARGO_TARGET_NOTE_PREFIX`）を残す。試験は `crates/task-dispatch/src/dispatcher/tests/target_env.rs` の `target_env_repos_empty_child_run_uses_the_scratch_target`・`target_env_repos_empty_child_work_unit_check_uses_the_scratch_target`・`target_env_integration_check_uses_the_task_scratch_target`・`target_env_missing_ancestor_checkout_is_recorded`（接頭辞は表の `cargo_target_env_` でなく `target_env_`）。

### D2. repo 直下 target の掃除と所有関係

既定猶予は6時間を維持する。掃除候補は終端 task の所有 checkout とその WU に限り、running run・生存 task・build lock があれば保護する。子 task の終端で親 checkout を削除せず、checkout owner と参照する木の task を追跡し、最後の利用者が終端して猶予を経た時点で候補化する。配送前は候補を記録して安全性を検査し、配送後は merge/checkout が完了した owner checkout を再走査する。cron は有効化状態、apply 実行結果、削除/保護理由を記録し、dry-run のまま成功扱いにしない。

#### D2 実装の付記（2026-10-10、repo-target-gc）

- 木の判定は `task_worker::workspace_targets::tree_hold`（`running_run` / `active_descendant` / `grace`）。猶予は木の最後の終端から数える。
  `scan_finished_task_targets`・`finished_task_targets`・`workspace_prune::find_prune_candidates` が使う。
- cron に依らない常設の掃除: `Dispatcher::sweep_repo_targets`（tick、600 秒ごと、`workspace_target_after_hours` 既定 6）。
  I/O は `task_dispatch::target_sweep::{repo_target_gc_move_aside, remove_moved_targets}`（lock を持った rename → 別スレッドで削除、
  量は `st_blocks` と `statvfs` 差分）。cron の `sweep_workspace_targets` も同じ関数を使い、残した理由を `skipped` に出す。
- 試験: task-worker `repo_target_gc_finished_task_target_is_removed`・`repo_target_gc_running_descendant_keeps_the_parent_target`・
  `repo_target_gc_cargo_lock_keeps_the_target`、task-dispatch `repo_target_gc_removes_the_target_of_a_finished_task`・
  `repo_target_gc_keeps_the_target_while_a_descendant_runs`・`repo_target_gc_keeps_the_target_while_cargo_holds_the_lock`・
  `repo_target_gc_tick_removes_a_finished_task_target_without_cron`。

### D3. release-build lease の prune と symlink 回復

各 release の開始時刻を target marker に原子的に記録する。梱包後、直前 build 開始時刻より古い workspace crate の test binary と対応 `.d`、および live 参照のない hash 世代を削除する。依存 crate の rlib は保持する。lease size が設定上限を超えたら target を空にして seed から再作成し、seed が無い場合も空 target から作り直して gate をやり直す（長時間 gate を許容し、timeout 内で終わらなければ失敗として報告）。`browser-ledger.sh` は `release.sh` と同じ `sd_scratch_lease` 解決を使う。GC/prune/recreate 時に `.cargo-target` が dangling または古い lease を指していたら安全な fallback/現 lease へ symlink を修復する。

#### D3 実装付記（release-prune）

`scripts/selfdeploy/lib.sh` の `sd_release_prune_record_start` は `.celeris-release-build-start` を一時ファイルから rename して更新する。`sd_release_prune_stale_test_binaries` は workspace crate 名と `debug/deps` の `.d`/対応 hash binary を照合し、前回 marker より古い組だけを削除する。依存 `.rlib`/`.rmeta` と build script 出力は対象外。`sd_release_prune_enforce_limit` は `SD_RELEASE_TARGET_MAX_BYTES`（既定 68719476736 bytes）を超えた target を作り直し、任意の `SD_RELEASE_TARGET_SEED` の内容をコピーする。`release.sh` は lease 決定後、workspace clean と gate より前にこれらを呼ぶ。回帰試験は `scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh`。

#### 付記 2026-10-10: release prune の大きさ判定

`sd_release_prune_size` は `btrfs filesystem du -s --raw <target>` が成功し、出力を認識できる場合、Exclusive と Set shared の合計を上限判定に使う。共有 extent は Set shared に一度だけ加算されるため、reflink seed の共有を各 target の全量として重複計上しない。btrfs が無い・失敗する・出力を認識できない場合は、走査した通常 file の inode を一度ずつ数え、`st_blocks * 512` を合計する。後者は sparse file の未割当領域と hardlink の重複を除け、非 btrfs host でも見かけのサイズによる誤った作り直しを避けられる。一方、非 btrfs で reflink の共有 extent を判別できないため、その場合は共有分を重複計上し得る。

#### 付記 2026-10-10: release prune の対象

final review（2026-10-10 08:28Z）の指摘: D3 実装付記の刈り込みは `crates/*/Cargo.toml` の package 名に一致する binary だけを候補にしていたので、integration test の binary（test file 名で作られる `notify-<hash>`・`api_scenarios-<hash>` など、workspace に 149 本）と crates/ の外の member（`tests/e2e`）を刈らず、量の約 9 割が残っていた。採った判定（`scripts/selfdeploy/lib.sh` の `sd_release_prune_stale_test_binaries`）:

- **候補名**: `cargo metadata --no-deps --format-version 1 --offline --manifest-path <build tree>/Cargo.toml` の全 package の全 target（kind を問わない。test・bin・lib・bench・example・custom-build）の名前と package 名の和（`-` は `_`）。cargo metadata が失敗したら、tree 内の全 `Cargo.toml`（`target`・`.git`・`node_modules` を除く）の `[package] name` と、その member の `tests/*.rs`・`src/bin/*.rs`・`benches/*.rs`・`examples/*.rs`（と `*/main.rs` の dir 名）から作る。log に `names from metadata|fallback` を出す。
- **対象 file**: `debug/deps/<name>-<hex 8 桁以上>.d` ごとに、同じ stem の実行ファイル・`lib<stem>.rlib`・`lib<stem>.rmeta` と `.d` 自身。`.d` と全ての出力が marker（直前 release の build 開始時刻）より古い組だけを消す。
- **依存 crate の除外**: 名前が一致しても `.d` の本文に `/registry/src/` か `/git/checkouts/` が出るものは依存 crate とみなして残す（例: workspace の test `notify` と依存 crate `notify`）。`debug/build/` の build script 出力・`.fingerprint` は触らない。
- **dry run**: `SD_RELEASE_PRUNE_DRY_RUN=1` なら消さずに `would remove <n> files (<bytes> bytes allocated)` を log に出す。大きさは `st_blocks * 512`（見かけの size でない）。
- 試験: `scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh` に integration test（`crates/example/tests/notify.rs` → `notify-<hash>`）、crates 外 member（`tests/e2e/tests/api_scenarios.rs`）、registry と git checkout を参照する同名の依存 crate（残る）、marker より新しい binary（残る）、metadata にだけ出る `[[test]] name`、dry run を足した。

残る誤差:

- 依存の判定は `.d` の path 文字列に頼る。`CARGO_HOME` を変えた registry でも path は `registry/src` / `git/checkouts` を含むので当たるが、`[patch]`・path 依存で tree の外の源を指す crate は名前が workspace target と一致したときに刈られる（次の build で作り直されるだけで、壊れはしない）。
- 候補名は**今回の** tree から作る。消えた test file・改名した target の古い binary は名前が一致せず残る（D3 の上限で作り直すまで）。
- marker が無い target（本変更を初めて配備した直後）は start=0 として何も刈らず、その release で marker を書く。刈り込みは配備後 2 回目の release から効く。
- `.d` の mtime が marker より古くても、cargo が fingerprint 一致で再利用した binary は mtime が更新されないので、今回の build で使った古い binary を刈ることがある。その場合 cargo は次の build で作り直す（依存の再 build は起きない。workspace crate だけ）。
- 実測（2026-10-10 08:4xZ、本番 `/local/celeris/data/scratch/targets/release-build/target` を読み取りだけで dry run。start = 進行中の release の開始 08:40:45Z）: 候補 307 file・25.2 GiB。workspace の実行ファイルのうち古いものは 75 個・18.5 GiB で、旧規則（package 名だけ）が拾えたのは 8 個・3.1 GiB。記録は進捗 `agent-docs/progress/2026-10-10-local-disk-growth-paths/prune-workspace.md`。

### D4. DB backup の保持と削除前検査

promote 前 backup は直近10本を既定保持し、rollback 用 backup は直近3本を別枠で保持する。定期 backup は直近48時間分に加え、日次7本・週次4本（UTC日/ISO週ごとの最新）を残す。全 `.sqlite3` backup の使用量上限は64 GiBを既定とし、超過時は保持集合の古い periodic から先に削除し、次に promote の古いものを削除する。rollback 用直近3本と各種の最新1本は保護する。削除を始める前に最新 backup 1個を read-only で `PRAGMA integrity_check` し、結果が `ok` 以外または検査失敗なら削除を一切行わない。既存 `backup_keep=48` は互換のため直近 hourly 本数として扱う。

#### 実装付記（backup-retention、2026-10-10）

実装の記録は `agent-docs/progress/2026-10-10-local-disk-growth-paths/promote-prune.md`（promote 側）と `periodic-retention.md`（定期側）。本節の名前は両葉の実装と試験から取った。

- promote 側: `scripts/selfdeploy/prune-backups.sh <backups_dir>` が `<YYYYMMDD-HHMMSS>-pre-<12 桁 hex>.sqlite3` を既定 10 本、`<TS>-pre-rollback.sqlite3` を既定 3 本（別枠）残す。上書きは環境変数 `CELERIS_PROMOTE_BACKUP_KEEP` / `CELERIS_ROLLBACK_BACKUP_KEEP`（正の整数のみ。不正値は何も消さず exit 1）。`scripts/selfdeploy/promote.sh` は成功末尾で `sh prune-backups.sh "$SD_BACKUPS"` を呼ぶ（失敗は warning のみで昇格は失敗にしない）。削除候補のある型ごとに残す最新 1 個へ `sqlite3 'file:<path>?mode=ro' 'PRAGMA integrity_check;'` をかけ、どれか 1 つでも `ok` でなければ両型とも 1 本も消さない。試験は `scripts/selfdeploy/tests/promote_backup_retention.sh`（引数なし、一時 dir と sqlite3 だけ）の `promote_backup_retention_default_keeps_10`・`_env_override`・`_rollback_separate_3`・`_other_files_untouched`・`_integrity_failure_deletes_nothing`・`_promote_calls_prune`。
- 定期側: `crates/celeris/src/db_maintenance.rs` の `plan_backup_retention`（時刻と一覧を引数で受ける純関数。直近 `keep` 本・48 時間以内の全世代・UTC 日次・ISO 週次を残す）、`prune_backups` → `prune_backups_with_check`（`integrity_check` を注入できる形）、`integrity_check`（read-only の `PRAGMA integrity_check`）。最新の認識済み backup 1 個が `ok` でなければ警告を出して刈り込みを行わない。合計上限は同じ経路で、定期 backup を先に、次に promote 型の古いものから削る。各型の最新 1 本と rollback 直近 3 本は保護する。設定は `config/celeris.example.toml` の `[db]` にある `backup_daily_keep`（既定 7）・`backup_weekly_keep`（既定 4）・`backup_max_total_bytes`（既定 64 GiB）。`backup_keep`（既定 48）は直近 hourly 本数。既定値は `crates/celeris/src/config/db.rs` の `default_db_backup_*`、読み込みの試験は `crates/celeris/src/config/tests.rs` の `db_accepts_both_the_bare_path_string_and_the_table_form`。試験は接頭辞 `backup_retention_` で `crates/celeris/src/db_maintenance/tests.rs` の `backup_retention_daily_and_weekly_generations_are_kept`・`_keeps_every_backup_inside_48_hours`・`_total_limit_prunes_periodic_before_promote`・`_protects_latest_per_kind_and_three_rollbacks`・`_integrity_failure_skips_all_deletions`・`_ignores_other_file_names`（時計は注入）。
- 差分（本付記で確定した事実）: 合計 byte 上限は定期側の刈り込みだけが見る。`prune-backups.sh` は本数だけを見るので、promote 側だけで上限を超えても上限による削除は起きない。`rollback.sh` は `pre-rollback` backup を作るが prune を呼ばない（次の promote で刈られる）。

### D5. disk 使用量の見積り

filesystem の圧迫は同一 mount の `statvfs` 空き容量差分で評価する。lease/target 単位は保存済み `size_bytes` または block 数を使い、再帰 `du` を各観測で起動しない。共有 filesystem の statvfs 差分は他利用者の書込みも含むので、個別 lease の帰属量と混同しない。

#### 付記 2026-10-10: scratch の上限と測り方

人のコメント（2026-10-10）の (2)(3) への決定。実装は後続葉 `sizing-impl`（試験接頭辞 `scratch_shared_`）。

**事実（2026-10-10 に確認）**

- `[scratch] seed_reflink` は既定 `false`（`crates/celeris/src/config/scratch.rs`）で、本番 config にも書かれていない。`seed_housekeeping`（`crates/task-dispatch/src/dispatcher/housekeeping.rs`）は `seed_reflink = false` で即 return し、`allocate_with_seed` の `SeedPolicy.enabled` も false になる。このため seed は 1 度も作られず、全 owner の target は空から build している。
- `/local` は btrfs 1 つ（`/dev/mapper/pve-celeris--local`、`compress=zstd:1`、`subvol=/`）。scratch pool（`/local/celeris/data/scratch`）、作業場所、run の `$TMPDIR` は同じ filesystem（`stat -f` の id `72c118bd24a3e9a4`）。
- worker の `$TMPDIR` で自分の一時 file だけを使って試した（4 MiB）: `cp --reflink=always` は `failed to clone: Operation not permitted` になる。python から直接呼んだ ioctl `FICLONE` も `EPERM` だった（worker は seccomp filter の中で動いている。`/proc/self/status` の `Seccomp: 2`）。一方、`cp --reflink=auto` と `copy_file_range(2)` は成功した。写した file の FIEMAP は全 extent に `FIEMAP_EXTENT_SHARED`（0x2000）があり、物理位置が元 file と一致した。`cp --reflink=never` の写しには SHARED が無かった。圧縮 extent（`FIEMAP_EXTENT_ENCODED` 0x8）も共有される。
- つまり `probe_pool_share`（`cp --reflink=auto` のあと FIEMAP の SHARED を見る。`--reflink=always` の成否は判定に使わない）は、この filesystem で成功する。btrfs の `copy_file_range` は clone なので、FICLONE が拒まれる環境でも共有になる。probe と seed の写しは daemon の process で動くので、worker の seccomp は関係しない。
- 測り方: `measure_tree`（`crates/task-worker/src/scratch/gc.rs`）は path ごとに `st_blocks * 512` を足す。reflink で共有された extent は owner の数だけ重ねて数え、hardlink（cargo が `target/debug/<bin>` を `deps/` から張る）も path の数だけ数える。2026-10-10 の実測では、生きている target は 4 個で lease の `size_bytes` の合計は 106.4 GiB（release-build 68.0、task 23.3、wu 15.2、ほか 0）。release-build の hardlink の重複は 2.4 GiB だった。seed が無いので、reflink による重複は今は 0。

**決定 S1: `seed_reflink` は有効にできる**

有効にする条件は 3 つ。(a) `[scratch] dir` が reflink を共有できる filesystem（btrfs・XFS reflink=1）の上にあること。daemon が起動後最初の seed 確認で `probe_pool_share` を実行し、失敗したら warn `scratch: seed refresh disabled; pool cannot share extents` を出して seed を作らない（fail-closed）。(b) 測り方 S3 が入った release であること。旧来の測り方では seed（約 15〜20 GiB）を owner とは別に数えるうえ、seed から写した owner も全量で数える。このため、有効にしただけでは watermark への距離が縮む。(c) 空きが `seed_refresh_hold` を満たすこと（pressure 中は seed を作り直さない）。seed の build（`cargo build --workspace --all-targets`）は main が進むたびに daemon の thread で 1 本走る。人の手順・確認・戻し方は `docs/ops/local-disk-growth.md` §7 に書く。既定値は `false` のままにする（filesystem に依存するため。host ごとに人が有効にする）。

**決定 S2: `targets_max_gb` の既定は 160**

- 内訳: release-build の lease は pin（GC の対象外）で、`SD_RELEASE_TARGET_MAX_BYTES` の既定 64 GiB を超えると作り直す（D3）。task の target は実測で 12〜25 GiB（2026-10-10 の lease 237 件の中央値は約 15 GiB、最近の木で 20〜24 GiB）。同時に動く coding owner を 4 本と見ると、64 + 4 × 20 = 144 GiB が high watermark（0.90）の位置になる。160 × 0.90 = 144、160 × 0.70 = 112（GC 後に release-build と 2 本が残る）。
- `/local` 300 GiB との収支: pool の外の使用量は 2026-10-10 に約 91 GiB（作業場所・DB backup ≤ 64 GiB（D4）・release・その他）。160 + 91 = 251 GiB で、`min_free_disk_mb`（5 GiB）の 2〜3 倍の余白も残る。足りなくなったときは、既存の `effective_max`（ADR-0075 D1）と disk_watch が statvfs の空きで縮める。
- 既定 100 は release-build の pin が 64 GiB まで育つ前提と両立しない（pin だけで low watermark 70 GiB に近く、task 1〜2 本で high 90 GiB に達して GC が pin を消せない）。本番の暫定値 150 は、この決定の値に置き換える。
- `total_max_gb` の既定は `targets_max_gb + l1_max_gb` = 200 にそろえる（現在の 150 は targets 160 より小さく、実効上限の警告が常に出るため）。
- 既定値の置き場所は `crates/celeris/src/config/scratch.rs` の `default_scratch_targets_max_gb`・`default_scratch_total_max_gb` と `crates/task-worker/src/scratch.rs` の `ScratchSettings` 既定。`config/celeris.example.toml` の注記も合わせる。

**決定 S3: 共有 extent は pool 全体で 1 回だけ数える（FIEMAP の物理 extent で重複除去）**

候補は 3 つあった。(i) FIEMAP の物理 extent で pool 全体の重複を除く、(ii) SHARED の extent を別勘定にして owner には数えない、(iii) statvfs の差分。この中から (i) を選ぶ。(ii) だけでは、seed を消したあとに残る共有分が誰にも数えられない。(iii) は owner ごとに帰属できず、pool の外の書き込みも混ざる（D5）。statvfs は (i) の上限の検算にだけ使う。

- 置き場所: FIEMAP の走査は `crates/task-worker/src/scratch/reflink.rs` に汎用の `fiemap_extents(path, &mut dyn FnMut(FiemapExtentInfo))` を足す（既存の `fiemap_all_shared` と同じ ioctl。測定では `FIEMAP_FLAG_SYNC` を付けない）。owner 1 個を測る `measure_tree_shared(path, &mut SharedExtentIndex, owner_id, budget) -> io::Result<OwnerMeasure>` は `crates/task-worker/src/scratch/gc.rs` に置く。`SharedExtentIndex` は pool 全体で共有する in-memory の表で、`fe_physical` から (長さの最大値, その extent を参照する owner の集合) を引く。`crates/task-dispatch/src/scratch_gc.rs` の `measure_one` は、`SizeCache` と同じ `Arc<Mutex<…>>` で持つこの表を測定スレッドに渡す。測定は従来どおり 1 回に 1 owner。
- 数え方: SHARED の付かない extent と `FIEMAP_EXTENT_DELALLOC`/`UNKNOWN`（物理位置が未確定）の extent は、その owner の `exclusive_bytes` に足す。SHARED の extent は表に登録する（その owner の前回分は先に外す）。inode は owner の中で (dev, ino) を 1 回だけ数える（hardlink の重複を除く）。pool の使用量 = Σ `exclusive_bytes` + 表の長さの合計。GC の `GcEntry.estimated_bytes`（消して空く量）は `exclusive_bytes` + その owner だけが参照する共有 extent の量にする。lease には従来の `size_bytes`（＝ `exclusive + shared` の上限値。互換）に加えて `exclusive_bytes`・`shared_bytes`・`method`（`fiemap` | `blocks`）を書く。
- 検算: GC の `used_bytes` は min(上の合計, statvfs の used) にする（pool が filesystem 全体の使用量を超えることはないため）。
- 計算量の上限: FIEMAP の ioctl は 64 extent ずつ。1 owner あたり extent は 400 万個まで、壁時計は 300 秒まで（`MeasureBudget`）。表の entry は pool 全体で 800 万個まで（1 entry 約 32 byte で約 256 MiB）。どれかを超えたら、その owner は fallback にする。参考: release-build 68 GiB は zstd の 128 KiB extent で約 55 万個、task target 20 GiB は約 16 万個。
- 失敗時の fallback: FIEMAP が `ENOTTY`/`EOPNOTSUPP`（tmpfs・NFS など）を返したとき、上限を超えたとき、ioctl が失敗したときは、その owner を従来の `st_blocks * 512` の合計（hardlink の重複は除く）で測る。その owner の SHARED 分は表に登録せず、全量を `exclusive_bytes` として数える（上限側に倒れるので、GC が消さなすぎることはない）。lease の `method` は `blocks` にし、debug log に理由を残す。daemon の再起動直後で表がまだ空のときは、未測定の owner に lease の `size_bytes`（上限値）を使う。全 owner を 1 巡測ったあとは表の値を使う。
- release-build の `sd_release_prune_size`（shell。btrfs-progs があれば `btrfs filesystem du`）は 2026-10-10 付記のまま変えない。

## 実装の分担

| 決定 | 後続葉 | 主な担当 file | 試験名の接頭辞 |
|---|---|---|---|
| D1 | `worker-target-env` | `crates/task-dispatch/src/dispatcher/{worker_task.rs,work_units.rs,phase_integration.rs,review_spawn.rs,housekeeping.rs,workspaces.rs}`、`crates/task-worker/src/build_cache.rs` | `cargo_target_env_`、既存 `worker_task_` / `work_unit_` / `review_` 系 |
| D2 | `repo-target-gc` | `crates/task-worker/src/{workspace_targets.rs,workspace_prune.rs}`、`crates/task-dispatch/src/{target_sweep.rs,dispatcher/maintenance.rs}`、`crates/celeris/src/config/maintenance.rs` | `workspace_target_`、`target_sweep_workspace_` |
| D3 | `release-prune` | `scripts/selfdeploy/{release.sh,lib.sh,browser-ledger.sh}`、`crates/task-worker/src/target_sweep.rs`、`crates/task-dispatch/src/target_sweep.rs`、`crates/celerisctl/src/commands/target_sweep.rs` | shell `release_prune_`、Rust `target_sweep_release_` |
| D4 | `backup-retention` | `scripts/selfdeploy/{promote.sh,lib.sh,rollback.sh}`、`crates/celeris/src/{db_maintenance.rs,config/db.rs,config/tests.rs}` | Rust `backup_retention_`、shell `promote_backup_retention_` |
| D5 | `release-prune`, `repo-target-gc`, `backup-retention` | 各担当の size accounting と disk-watch 呼出元 | `target_sweep_*`、`backup_retention_*`、`disk_watch_*` |

## 運用上の反映

本番 host の lease、backup、symlink、cron、config の変更や daemon 再起動はこの ADR では行わない。実装・review・release 後、人が変更内容と対象を確認して実施する。人の手順は対象 backup の integrity check、現 lease と symlink の解決先、`statvfs` の前後値、service health を確認し、異常時に削除・promote を止めるものとする。

#### 付記 2026-10-10: 本番相当の削減量の試験

本番 `release-build/target/debug/deps` を読み取り専用で採取した inventory と、その縮尺 fixture を `scripts/selfdeploy/tests/fixtures/release-build-deps-inventory.tsv` / `scripts/selfdeploy/tests/release_prune_production_scale.sh` に記録する。試験は fallocate した実 block と `.d` を作り、同じ prune 関数を呼ぶ。registry/git 由来の依存と marker より新しい binary が残ることも確認する。

採取は 2026-10-10 09:36Z、marker は直前 build の 08:40:45Z。採取時点では marker より古い workspace 実行 binary は 11 個・102,502,400 allocated bytes（約 0.095 GiB）だった。08:40Z の以前の dry-run 記録（381 個・108 GiB の test binary 全体、うち古い候補の記録 75 個・18.5 GiB）は同一時点の inventory ではないため、本試験結果と混同しない。試験 fixture は縮尺後の block rounding があり、実測換算との差分を出力する。
