# PROGRESS — Phase G（ビルドキャッシュの 2 層化）

目次は `docs/PROGRESS.md`。設計は `docs/adr/0075-tiered-build-cache.md`（ADR-0075、Proposed）、入力は
`docs/notes/build-cache-tiering-input-2026-09-28.md`（人の方針）。

## Phase G0（設計）— 完了 2026-09-28

docs 配下だけの変更（コードは変更しない）。作業は git worktree の中（main へ merge / push しない）。

### 設計の要点

- **D1 レイアウトと容量**: ローカルの `/var/lib/celeris/scratch/{targets/<owner>/{lease.json,target/}, sccache-l1/}`。owner は
  `task-<id>` / `task-<id>/wu-<work_unit_id>`（WU の key ではなく行の id。F5-fix と同じ理由）/ `release-<sha12>` / `agent-<name>`。
  上限 `[scratch]` の既定は targets 100G・L1 40G・合計 150G。ただし実効上限 = min(150G, filesystem から pool の外の使用量と
  `min_free_disk_mb` を除いた分)。DB と同居する間は `min_free_disk_mb` を最後の砦として残す。Proxmox 側は `mp1`（専用ボリューム）を第一案として
  提案（人の判断）。L2 は NFS の `~/.local/celeris/cache/sccache-l2/<k0k1>/<key>.zst`。scratch が NFS 上なら起動時に無効化。
- **D2 semantic GC**: tick の `scratch_gc`（決定論的、LLM 無し、削除は rename → 別スレッド）。分類は P0 pinned（running / reviewing、進行中の WU、
  TTL 内の外部 lease。絶対に消さない）/ P1 waiting（48h）/ P2 retry（24h）/ P3 completed / seed（repo ごとに最新 1 つ）。削除順は legacy・野良 →
  P3（LRU）→ seed → P2 → P1。high 0.90 → low 0.70。「近い commit の保持」は seed + adopt（rename で引き継ぎ）で実現し、git の距離は tick では
  計算しない。release.sh と実装エージェントは `celerisctl scratch lease/touch/release` の lease ファイルで同じ GC に載る。
- **D3 割り当て**: 全ての cargo 経路（worker / WU / 統合 / reviewer checks / release gate / 実装エージェント）に
  `CARGO_TARGET_DIR=<scratch>/targets/<owner>/target`。adopt は「候補の最終書き込み < 引き継ぐ側の checkout 時刻」のときだけ（cargo の mtime
  fingerprint の誤認〈F5-1 の E0609〉を避ける安全条件）。空き不足は緊急 GC、それでも足りなければ既存の「ディスク不足 (infra)」で dispatch を保留。
  WU 終端で即回収（F5-fix と整合）。
- **D4 sccache L1**: `RUSTC_WRAPPER` / `SCCACHE_DIR=<scratch>/sccache-l1` / `SCCACHE_CACHE_SIZE` / 専用 port。server は `celeris@` の外の
  専用 unit（ADR-0060 の教訓）。導入は `tools/sccache/VERSION` + `scripts/scratch/setup-sccache.sh`（人が 1 回）。Celeris の run は
  `CARGO_INCREMENTAL=0`（依存は元々非 incremental なので sccache に載る。主な得は incremental/ の容量）と
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only` を既定にする。
- **D5 L2 = (b) webdav cache server**（`crates/scratch-cache`、`celeris cache-server`、loopback）。理由: miss のとき L2 を見て L1 へ promote
  する真の階層は (b) だけ。(a) の promote は key → repo の対応が無く対象を選べない。(b) は sccache の公開 backend 契約だけに依存する。L2 の I/O を
  有界スレッドとタイムアウトに閉じ込めれば NFS 停止時も L1 だけで動く。核が純粋な Rust でテストしやすい。
- **D6 監視**: `celerisctl scratch status [--json]`、`GET /api/v1/metrics/scratch`、GUI デーモン画面に 1 行、journal（tracing）。
- **D7 移行**: `build_cache_dir` は `[scratch] dir` の既定の基準としてだけ残す。既存の `build-cache/cargo/*` と
  `/var/lib/celeris/release-build/.cargo-target` は初回 GC で（1h 以上 idle のものだけ）削除。`~/.cargo/config.toml` と `/tmp` に target を
  置かない規約。`SD_CARGO_TARGET` を scratch の lease にする。実装エージェントの定型文（`eval "$(celerisctl scratch env --owner agent-<name> …)"`）。

### G0 時点の観測（読み取りだけ）

- `df /` = 252G 中 152G 使用・91G 空き。rmaeda から見えるのは `/tmp/agent-platform-f5-1-target` 32G（scratch の外の野良 target）、
  `/var/lib/celeris` 11G、`/usr` 2.9G 程度で、残り約 100G は見えない（root 専用領域か NFS の mount の下の旧データ。未確認）。
- `which sccache` → 空（未導入）。`~/.cargo/config.toml` に target-dir は無い（2026-09-28 に人が廃止、linker 設定のみ）。
- F5-fix（`worktree-agent-aea5cf7690b5b2bc5`、879d01c / 701ee5f / e7a0f23）は main に未 merge。

### Phase G1 の受け入れ条件（ADR-0075 §5 の写し）

前提: F5-fix が main に merge 済み。

1. 全経路（Task 単位の run、v2 の WU の run と checks、統合 WU の検査、reviewer の checks）の `CARGO_TARGET_DIR` が
   `<scratch>/targets/<owner>/target` で、`request.json` の `cargo_target_dir` と一致する。コンテナ・Remote には与えない。
2. `plan_gc` が P0 を選ばず、削除順（legacy / 野良 → P3 LRU → seed → P2 → P1）を守り、low watermark で止まる。
3. WU の終端で、WU の target が次の tick で rename → 別スレッドで削除される（seed を除く）。`failed` / `blocked` の WU は残る。
4. adopt は安全条件を満たすときだけ起きる。満たさなければ空から始める。
5. 空き < `min_free_disk_mb` → 緊急 GC → 回復まで dispatch 保留 +「ディスク不足 (infra)」通知 1 回、回復で自動解除。
6. 外部 lease の TTL 切れ・release 後に P3 になる。`celerisctl scratch env` と dispatcher の env が一致する。
7. scratch dir が NFS なら無効化して従来の挙動。`[scratch] enabled = false` で F5-fix の挙動に戻る。
8. `celerisctl scratch status --json` と `GET /api/v1/metrics/scratch` が同じ schema、GUI に 1 行。schema / 生成型の再生成で差分ゼロ。
9. `release.sh` が lease を取り、終了時に release する（`scripts/selfdeploy/tests` の偽 celerisctl で確認）。
- 全体ゲート: `cargo fmt --all -- --check`、`cargo test --workspace --no-fail-fast` FAILED 0、`cargo clippy --workspace --all-targets -- -D warnings`
  警告 0、GUI の typecheck / lint / test / gen:types 差分ゼロ。

### 検証（G0）

- `git status --short` → `docs/adr/0075-tiered-build-cache.md`、`docs/progress/phase-G.md`、`docs/PROGRESS.md` だけ（docs 以外の差分なし）。
- コードに触れていないので `cargo test` / `cargo clippy` は実行していない（G0 は設計のみ）。

### 未解決（ADR-0075 §4）

- U1: sccache の Rust の key が絶対パス（worktree、`--out-dir`、`-L dependency=`）を含むか → G2 の最初に実測。依存の hit が出なければ止めて人に聞く。
- U2: 実効上限。見えない約 100G の正体（人が root で `du -x`）と `mp1` 追加の判断（人）。
- U3: `CARGO_INCREMENTAL=0` による壁時計の悪化（G2 で測る）。
- U4: L2 の事前 promote の要否（G3 の測定後）。
- U5: sccache の webdav backend の発行メソッド・backend 停止時の挙動・起動時の storage check（G3 の最初に loopback の stub で記録）。
- U6: clippy-driver と sccache の組み合わせ（未確認）。
- U7: F5-fix の main への merge が G1 の前提。

### 提案

- 人へ: G1 の前に F5-fix を merge し、`/tmp/agent-platform-f5-1-target`（32G）を消してよいか判断してほしい（本 Phase では触っていない）。
  `mp1` の専用ボリュームを足すなら `/var/lib/celeris/scratch` に mount する（Celeris の設定は同じパスのまま）。

## ルートディスクの「見えない 110 GB」の正体（2026-09-28、人と Fable）

- `du -x /*` では 23 GB しか見えないのに `df` が 137 GB を示していた原因は `/.migration/home.raw`（見かけ 1 TiB のスパース raw、実使用 115 GB。2026-09-25 のホーム移行時にホスト側から作られたコピー。所有者がコンテナの uid 範囲外のため CT の root でも削除不可）。Proxmox ホストから `rm -rf /proc/<init-pid>/root/.migration` で削除 → `/` は 137 GB → 22 GB（空き 220 GB）。
- これで ADR-0075 D1 の scratch pool（targets 100 GB + L1 40 GB）はルート LVM に収まる。U2 は解消（`mp1` の追加は不要になった。LVM 拡張も当面不要）。
## Phase G1（scratch pool + 割り当て + semantic GC + watermark + celerisctl / metrics）— 着手 2026-09-28

ADR-0075 の状態を Accepted にした。作業は git worktree の中（main へ merge / push しない）。

### G1 checkpoint 1: `scratch.rs` の純粋部 + `[scratch]` 設定（完了 2026-09-28）

- `crates/task-worker/src/scratch.rs`（新規）: owner（`task-<id>` / `task-<id>/wu-<id>` / `release-<sha12>` / `agent-<name>`）と
  パス、`lease.json`（`celeris.scratch-lease/1`、`deny_unknown_fields`）の読み書き、`.lock` の flock、`allocate`（lease の作成・touch と
  adopt）、`touch` / `release`、`cargo_env`（dispatcher と `celerisctl scratch env` が共有）、`classify`（D2 の表）、`legacy_class`、
  `plan_gc`（純粋関数）、`choose_adopt`、`is_on_nfs` / `apply_nfs_check`、`measure_tree`。
- `crates/celeris/src/config.rs`: `[scratch]`（`enabled` / `dir` / `targets_max_gb` / `l1_max_gb` / `total_max_gb` /
  `high_watermark` / `low_watermark` / 保持期限 / `gc_max_per_tick` / `adopt` / `adopt_max_distance` / `measure_interval_secs`）。
  `dir` の既定は `build_cache_dir` の親の `scratch/`。`scratch_settings()` が NFS の検査をして `DispatchConfig.scratch` に入れる。
- テスト: `cargo test -p task-worker --lib scratch` → 17 passed、`cargo test -p celeris --lib scratch` → 2 passed。

### G1 checkpoint 2: dispatcher / review の割り当て、WU 終端の回収、adopt、緊急 GC（完了 2026-09-28）

- `crates/task-dispatch/src/scratch_gc.rs`（新規）: pool の走査（lease と DB の状態で分類。木は辿らない）、`run_gc`（走査 → `plan_gc` →
  `.deleting-*` へ rename）、削除スレッド・測定スレッド（同時にそれぞれ 1 本、測定は間隔ごとに 1 つ）、legacy の根
  （`build_cache_dir/cargo/*`、`<releases_dir>/.cargo-target` の先）、実効上限、`ScratchStatus` の組み立て。
- `dispatcher.rs`: run の `CARGO_TARGET_DIR` を `CargoTargetPlan`（None / Legacy / Scratch）で決める。scratch なら run の開始時に
  `allocate_scratch_target`（lease の作成・touch と adopt。commit の距離はここだけで計算）。WU の checks・統合 WU の検査・reviewer の
  checks は `check_cargo_target_env` が同じ owner の lease を touch して env を返す。tick の `scratch_gc` phase が F5-fix の
  `cleanup_work_unit_build_caches` を置き換える（scratch が無効なら従来どおり呼ぶ）。`check_disk_space` は scratch の dir も見て、
  足りなければ緊急 GC → 保留 + 通知 1 回（消せるものが P0 だけなら本文に pinned の一覧）。watermark の到達・解除、GC、実効上限の縮小は
  tracing（journal）。`DaemonSnapshot.scratch` に観測値。起動時に NFS で無効化した理由を warn。
- テスト: `task_dispatch::dispatcher::tests::{every_cargo_path_uses_the_scratch_target_dir, terminal_work_unit_target_is_reclaimed_on_the_next_tick,
  low_disk_runs_emergency_gc_before_pausing_dispatch, scratch_on_nfs_falls_back_to_build_cache_dir,
  run_start_adopts_a_finished_target_that_predates_the_checkout}`、`shared_build_cache_is_not_applied_to_remote_workspaces`（scratch の計画を
  渡しても Remote には与えない）、`task_dispatch::scratch_gc::tests::*`（4 件）。
- `cargo test -p task-dispatch -p task-worker -p celeris -p task-ops --no-fail-fast` → FAILED 0。`cargo clippy --workspace --all-targets -- -D warnings` → 警告 0。

### G1 checkpoint 3: `celerisctl scratch`、`SD_CARGO_TARGET` の lease、定型文（完了 2026-09-28）

- `crates/celerisctl/src/commands/scratch.rs`（新規）: `status [--json]` / `gc [--dry-run]` / `lease <owner> --repo [--worktree] [--base] [--ttl]` /
  `touch` / `release` / `env <owner> [--repo …]`（owner は位置引数でも `--owner` でもよい）。`env` は dispatcher と同じ
  `task_worker::scratch::cargo_env`。scratch が無効なら `lease` / `env` は失敗（release.sh が従来の target に戻る合図）。
  `lease.json` に `ttl_secs`（`--ttl`）を足した。`build-cache prune` の help に「scratch へ移行済み。`celerisctl scratch gc`」。
- `scripts/selfdeploy/lib.sh`: `sd_celerisctl_bin` / `sd_scratch_lease` / `sd_scratch_touch` / `sd_scratch_release`。`release.sh` は
  `git worktree add` の直後に lease（`--repo "$SD_REPO" --worktree "$BUILD" --base <sha>`）→ 成功すれば `SD_CARGO_TARGET` をその target に、
  `trap` で終了時に release、各 step の前に touch。
- 定型文: `crates/task-worker/src/preamble.rs` の `shared_build_cache_note`、`.claude/agents/{implementer,auditor}.md`、
  `docs/ops/home-nfs-migration-2026-09-25.md` §5 の訂正（`~/.cargo/config.toml` に target-dir を置かない）。
- テスト: `celerisctl::commands::scratch::tests::{env_matches_the_dispatcher_env, lease_touch_release_round_trip, gc_dry_run_lists_without_removing,
  disabled_scratch_makes_lease_fail_so_release_sh_falls_back}` → 4 passed。`bash scripts/selfdeploy/tests/release_uses_scratch_lease.sh` →
  `release_uses_scratch_lease: ok`（偽の celerisctl / cargo / pnpm。lease → touch → release の順、gate の `CARGO_TARGET_DIR` が scratch、
  lease 失敗で `$SD_RELEASES/.cargo-target`）。

### G1 checkpoint 4: metrics / API / GUI の 1 行、schema と生成型（完了 2026-09-28）

- `task_ops::daemon::{ScratchStatus, ScratchOwnerView, ScratchLegacyView, ScratchGcView, ScratchGcRemovedView}`（`celeris.scratch-status/1`）と
  `DaemonSnapshot.scratch`（`#[serde(default)]`）。`GET /api/v1/metrics/scratch` はスナップショットの `scratch` を返す（無ければ 404
  `scratch_unavailable`）。`celerisctl scratch status --json` は同じ型を出す（daemon の直近の GC は持たないので `last_gc` は null）。
- `docs/api/v1/api-v1.schema.json`（`metrics_scratch` と `DaemonSnapshot.scratch`）と `gui/app/celeris/types.ts` を再生成して commit。
- GUI: `gui/app/routes/daemon.tsx` に `scratchLine`（「scratch 62 GB / 100 GB（pinned 18 GB、実効上限 150 GB）」。pressure が none 以外・実効上限の
  縮小・NFS で無効のときだけ注意色）を 1 行。
- テスト: `task_api::handlers::tests::metrics_scratch_matches_the_status_schema`（応答が `ScratchStatus` そのもの、キーが committed schema の
  properties と一致、スナップショット無しで 404）、`gui/test/unit/daemon.test.ts` の `scratchLine` 3 件。
- `cd gui && corepack pnpm@11.27.0 typecheck && lint && test && build` → exit 0、Test Files 73 passed / Tests 1116 passed。

### Phase G1 の全体ゲート（2026-09-28、完了）

- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace --no-fail-fast` → exit 0、passed 2543 / failed 0 / ignored 5（F5-fix 時点 2511 から +32）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、警告 0
- `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → ok、再生成後 `git status` 差分ゼロ
- `cd gui && corepack pnpm@11.27.0 gen:types`（差分ゼロ）`&& typecheck && lint && test && build` → すべて exit 0、Test Files 73 / Tests 1116 passed
- `bash -n scripts/selfdeploy/*.sh`（10 ファイル）→ 構文エラー 0。`bash scripts/selfdeploy/tests/*.sh` → 3 本とも exit 0

### 受け入れ条件（ADR-0075 §5 G1）との対応

1. 全経路の `CARGO_TARGET_DIR` = `<scratch>/targets/<owner>/target`、`request.json` と一致、Remote に与えない →
   `every_cargo_path_uses_the_scratch_target_dir`、`shared_build_cache_is_not_applied_to_remote_workspaces`。
2. `plan_gc` が P0 を選ばず順序と low watermark を守る → `plan_gc_never_selects_pinned_owners`、`plan_gc_follows_the_semantic_order_and_stops_at_low_watermark`、
   `plan_gc_keeps_one_warm_seed_per_repo`、`plan_gc_treats_unmeasured_owners_as_the_repo_maximum`、`legacy_build_cache_entries_are_reclaimed_only_when_idle`。
3. WU 終端で次の tick に rename → 別スレッドで削除（seed を除く）、failed / blocked は残る → `terminal_work_unit_target_is_reclaimed_on_the_next_tick`。
4. adopt は安全条件のときだけ → `adopt_requires_the_target_to_predate_the_checkout`、`adopt_prefers_the_nearest_commit_within_the_limit`、
   `adopt_skips_a_candidate_that_was_leased_again_after_the_scan`、`run_start_adopts_a_finished_target_that_predates_the_checkout`。
5. 空き不足 → 緊急 GC → 保留 + 通知 1 回 → 自動解除 → `low_disk_runs_emergency_gc_before_pausing_dispatch`。
6. 外部 lease の TTL 切れ・release で P3、`scratch env` と dispatcher の env が一致 → `external_lease_expires_after_ttl`、`lease_touch_release_round_trip`、
   `env_matches_the_dispatcher_env`。
7. NFS なら無効化・`enabled = false` で F5-fix → `scratch_on_nfs_falls_back_to_build_cache_dir`、`nfs_check_disables_scratch_with_a_reason`、
   `scratch_can_be_disabled`、`scratch_defaults_follow_the_build_cache_parent`。
8. `status --json` と `GET /api/v1/metrics/scratch` が同じ schema、GUI に 1 行、再生成で差分ゼロ → `metrics_scratch_matches_the_status_schema`、
   `gui/test/unit/daemon.test.ts`（scratchLine）、上のゲート。
9. release.sh が lease を取り終了時に release → `scripts/selfdeploy/tests/release_uses_scratch_lease.sh`。

ADR-0075 に「Phase G1 実装時の逸脱・明確化」1〜13 を追記した（`ttl_secs` の追加、測定は mtime を変えない、seed の近似、未測定の推定、
野良・legacy は測定した mtime で 1h 判定、lease 記録の 7 日の片づけ、checks は adopt しない、CLI の adopt 条件と `--worktree`、L1/L2 の欄は G2/G3、
`measure_interval_secs`、`[commands] setup` は対象外のまま など）。

### 未解決事項・G2 への申し送り

- **本番での確認（人）**: 昇格後に `celerisctl scratch status` で `/var/lib/celeris/scratch` が有効（NFS でない）こと、Task / WU の run の
  `runs/<run_id>/request.json` の `cargo_target_dir` が `/var/lib/celeris/scratch/targets/task-<id>[/wu-<id>]/target` であること、WU done の
  次の tick で WU の target が消えること、legacy の `build-cache/cargo/*` と `release-build/.cargo-target` が 1h idle の後に消えることを確かめる。
  **今動いている実装エージェントが手で指している `build-cache/cargo/agent-platform-<name>` も 1h 書き込みが無ければ回収される**（定型文の
  `celerisctl scratch env` に移ってもらう）。本番の config には `[scratch] dir = "/var/lib/celeris/scratch"` を明示することを勧める。
- **実効上限**: U2 のまま（見えない約 100G）。`scratch status` の `effective_max_bytes` と journal の「実効上限 N GB」で観測できるようになった。
- **G2 へ**: `task_worker::scratch::cargo_env` に sccache 系（`RUSTC_WRAPPER` / `SCCACHE_*`）と `CARGO_INCREMENTAL=0` /
  `CARGO_PROFILE_DEV_DEBUG` を足せば dispatcher（`check_cargo_target_env` と run の `with_env`）と `celerisctl scratch env` の両方に入る
  （今は run の経路は `CARGO_TARGET_DIR` だけを直接組んでいるので、G2 で `cargo_env` を使う形に揃える）。`ScratchStatus` に L1 の欄を
  `Option` で足す。`[commands] setup` にも env を与えるかを決める。
- seed の「使われている repo」は pool の中の P0〜P2 で近似（逸脱 3）。ready のまま lease を持たない Task の repo は数えない。

### 提案

- 人へ: 本番の `config.toml` に `[scratch] dir = "/var/lib/celeris/scratch"` を足す（既定でも build_cache_dir の親から同じ値になるが、明示
  しておくと `build_cache_dir` を動かしたときに pool が動かない）。`mp1` の専用ボリュームを足す判断（ADR-0075 D1）は変わらず人。

## G1 の本番反映（2026-09-28 03:4xZ）と、設定の新セクションが N-1 を壊す件

- 統合 main（G1: scratch pool / semantic GC / watermark / `celerisctl scratch` / metrics / release.sh の lease）。ゲート: fmt 0 / test FAILED 0（2543 件）/ clippy 0 / GUI typecheck・lint・test 1116 件・gen:types 差分ゼロ・build / selfdeploy tests ok。release `10bb975a731a`。
- verify の N-1（check 5）が false: 本番 config に先に足した `[scratch]` セクションを、旧バイナリ（ba2134a9fcc2）が `unknown field scratch` で拒否して起動できなかった。**設定に新しいセクションを足すのは、それを知るリリースが昇格した後**（さもないと N-1 と rollback が壊れ、旧デーモンの再起動も失敗する）。`[scratch]` を外して再 verify → live_ok → ライブ昇格。scratch は既定の dir（`build_cache_dir` の親 = `/var/lib/celeris/scratch`）で有効。
- 提案 P-G1-1: ADR-0075 D7 に上の順序を明記し、`release.sh` のゲートに「現行 config を N-1 の版でも parse できるか」を足す（rollback 可能性の確認）。

## Phase G2（sccache L1 の導入と Celeris の run への配線 + `CARGO_INCREMENTAL=0`）— 着手 2026-09-28

作業は git worktree の中（main へ merge / push しない）。自分のビルドは
`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-g2/target CARGO_INCREMENTAL=0`（lease は `celerisctl scratch lease --owner agent-g2 --ttl 21600`）。

### G2 checkpoint 1: U1 の実測（完了 2026-09-28）

測定の条件: sccache 0.18.0（`~/.cargo/bin/sccache`、`cargo install sccache`）、rustc 1.98.1、24 コア。専用の server
（`SCCACHE_DIR=/var/lib/celeris/scratch/sccache-l1-measure SCCACHE_CACHE_SIZE=20G SCCACHE_SERVER_PORT=4236 SCCACHE_IDLE_TIMEOUT=0 sccache --start-server`）。
ビルドは main ab8571c と同じ tree で `cargo build --workspace`、`CARGO_INCREMENTAL=0`、`RUSTC_WRAPPER=sccache`。各回の前に `sccache --zero-stats`。
target は `celerisctl scratch lease` で取った `agent-g2-{a,b,c,d}`（各回 空から）。壁時計は `date +%s%3N` の差。

| 回 | target の渡し方 | target / ソースの場所 | 壁時計 | Rust hit / miss | C/C++・asm hit / miss |
|---|---|---|---|---|---|
| A | env `CARGO_TARGET_DIR` | agent-g2-a / この worktree | 49.99 s | 0 / 192 | 0 / 382（L1 が空） |
| B | env `CARGO_TARGET_DIR` | agent-g2-b / この worktree | 45.13 s | **0 / 192** | 382 / 0 |
| C | `cargo build --target-dir`（env に無し） | agent-g2-c / この worktree | 45.93 s | 0 / 192（新しい key） | 382 / 0 |
| B' | `--target-dir`（env に無し） | agent-g2-b（空から）/ この worktree | 41.61 s | **163 / 29（84.9 %）** | 382 / 0 |
| D | `--target-dir`（env に無し） | agent-g2-d / **別のパスに展開した同じ commit**（`git archive HEAD`） | 41.76 s | **162 / 30（84.4 %）** | 382 / 0 |
| A' | env `CARGO_TARGET_DIR` + `CARGO_TARGET_DIR` を外して sccache を exec する wrapper | agent-g2-a（空から）/ この worktree | 45.67 s | **163 / 29（84.9 %）** | （wrapper の名前が `sccache` でなく cc-rs が使わなかった） |
| B'' | 同上 | agent-g2-b（空から、B' と同じパス）/ この worktree | **12.75 s** | **192 / 0（100 %）** | — |

- **原因（U1 の答え）**: owner をまたいだ Rust の hit が 0 だったのは、sccache 0.18 が Rust の key に **`CARGO_` で始まる env を全部**
  入れるから（`src/compiler/rust.rs` の `generate_hash_key` 8.。除外は `CARGO_MAKEFLAGS` / `CARGO_REGISTRIES_*` / `CARGO_BUILD_JOBS` /
  `CARGO_ENCODED_RUSTFLAGS` だけ）。cargo は自分の env を rustc にそのまま渡すので、owner ごとに違う `CARGO_TARGET_DIR` が全ての key を
  変える。`--out-dir` / `-L` / `--extern` のパスは key から除かれ（extern は中身の hash）、registry の依存は cwd も同じなので、
  **`CARGO_TARGET_DIR` を rustc の env から外せば依存は hit する**（B' / D / A'）。ソースの場所が違っても依存は hit する（D）。
- **残る miss（約 29）**: workspace のメンバーと、build script の `OUT_DIR`（target の中の絶対パス）を `env!` で読む crate とその下流
  （dep-info の env は値ごと key に入る）。同じ target のパスで作り直すと 100 % hit（B''）になることから、残りは target のパスに依存する分と
  判断した（`SCCACHE_LOG=debug` の server log でも miss の理由は出ない。crate ごとの内訳は**未確認**）。
- **basedir 系は Rust に効かない**: 0.18 の `SCCACHE_BASEDIRS` は C/C++ の preprocessor 出力の正規化にだけ使われる（`rust.rs` に参照が無い）。
  `--remap-path-prefix` は env の値を変えないので原因（`CARGO_TARGET_DIR`）には効かない。試した対策は「rustc に渡る env から
  `CARGO_TARGET_DIR` を外す」の 1 つで、依存の hit が出たので G2 を続ける。
- **壁時計の得は小さい**: 冷えた L1（A）50.0 s → 依存が hit（B' / D / A'）41.6〜45.7 s（−9〜17 %）。残りは workspace のメンバー
  （`task-dispatch` の 3 万行など、依存の後に直列で走る）と、キャッシュできない呼び出し 45（bin / proc-macro / build script の
  `crate-type`、42）とリンク。同じパスの再ビルド（B''）だけが 12.8 s。Celeris の run は owner ごとに新しい target なので、得は
  主に依存（Rust 163 件 + C/C++・asm 382 件）の CPU 時間。
- **cc-rs との関係**: cc-rs は `RUSTC_WRAPPER` のファイル名（stem）が `sccache` のときだけ C/C++ にも同じ wrapper を使う
  （`cc-1.4.6/src/lib.rs` の `rustc_wrapper_fallback`）。wrapper を挟むなら**ファイル名を `sccache` にする**（A' はこれを満たさず C のキャッシュを使っていない）。
- 実装への帰結（逸脱として ADR に書く）: `RUSTC_WRAPPER` は本物の sccache ではなく、Celeris が生成する小さな shell の wrapper
  `<scratch>/bin/sccache`（`unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR` → `exec <本物の sccache> "$@"`）にする。
- 測定用の target（`agent-g2-{a,b,c,d}`）と L1（`sccache-l1-measure`）、`git archive` の展開は測定後に削除（checkpoint 3 の後）。

### G2 checkpoint 2: env の配線（`cargo_env` に揃える）・`[scratch.sccache]` / `[scratch.cargo]`・status（完了 2026-09-28）

- `task_worker::scratch`: `SccacheSettings`（enabled / binary / server_port）・`CargoTuning`（incremental / dev_debug）を
  `ScratchSettings` に足した。`resolve_sccache`（scratch 無効 / `enabled = false` → disabled、バイナリが無い・server が応答しない →
  unavailable、それ以外 → wrapper を置いて ready）、`server_listening`（127.0.0.1:<port> への TCP 接続だけ。**sccache の client は
  呼ばない**〈server を起こしてしまう〉）、`wrapper_script` / `ensure_wrapper`（`<scratch>/bin/sccache`。U1 の結論どおり
  `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET_DIR` を外して本物を exec）、`sccache_server_env` / `sccache_env` / `cargo_tuning_env` /
  `cargo_env_with`（純粋）/ `cargo_env`（probe つき）。env の順は `CARGO_TARGET_DIR`、`CARGO_INCREMENTAL=0`、
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only`、`RUSTC_WRAPPER`、`SCCACHE_DIR`、`SCCACHE_CACHE_SIZE=<l1_max_gb>G`、`SCCACHE_SERVER_PORT`、
  `SCCACHE_IDLE_TIMEOUT=0`。
- dispatcher: run の経路（`run_worker` の scratch の分岐）を `cargo_env_with` に揃えた（G1 の申し送り。`spawn_blocking` の中で
  allocate → resolve → env）。checks（WU の checks・統合の検査・reviewer の checks）の `check_cargo_target_env` も `cargo_env`。
  legacy（scratch 無効）は従来どおり `CARGO_TARGET_DIR` だけ。起動ログに sccache の状態。スナップショットの `ScratchStatus.sccache`
  に状態（統計なし）。
- config: `[scratch.sccache]`（`enabled` 既定 true、`port` 既定 4236、`binary` 既定 `$CELERIS_STATE_DIR/tools/sccache/bin/sccache`）と
  `[scratch.cargo]`（`incremental` 既定 false、`dev_debug` 既定 `"line-tables-only"`、`""` で与えない）。どちらも書かなくても動く。
- `celerisctl scratch env` は `cargo_env`（dispatcher と同じ）、`env --server` は server の env と `CELERIS_SCCACHE_BIN`。
  `scratch status` に `sccache L1 <dir> (<state>) · <binary> · port` と、server が居れば `--show-stats --stats-format=json` の要約
  （hits / misses / Rust の hit / miss / サイズ）。
- `task_ops::daemon::{ScratchSccacheView, ScratchSccacheStats}`、`ScratchStatus.sccache`（`#[serde(default)]`）。schema と GUI の生成型を再生成。
- preamble の定型文に 1 行（`RUSTC_WRAPPER` などを上書きしない・server を起こさない）、`.claude/agents/{implementer,auditor}.md` に G2 の注記。
- テスト: `task_worker::scratch::tests::{sccache_env_is_complete_and_stable, sccache_env_is_omitted_without_binary_or_server}`、
  `task_dispatch::dispatcher::tests::{runs_get_sccache_env_when_the_server_is_up, runs_fall_back_to_plain_cargo_when_the_server_is_down}`、
  `task_dispatch::scratch_gc::tests::sccache_stats_summary_reads_the_json_shape_of_0_18`、`celeris::config::tests::scratch_cargo_defaults_disable_incremental`、
  `celerisctl::commands::scratch::tests::env_includes_sccache_when_the_server_is_up`。偽の server は loopback の port 0 に bind して accept
  するだけのスレッド、閉じた port は特権 port の 1（並行するテストと競合しない）。

### G2 checkpoint 3: 導入スクリプト・unit の雛形・手動 e2e（完了 2026-09-28）

- `tools/sccache/VERSION`（`0.18.0`）、`scripts/scratch/setup-sccache.sh`（既定は `cargo install sccache --locked --version <VERSION>
  --root $CELERIS_STATE_DIR/tools/sccache`〈target は scratch の `agent-setup-sccache`、終わったら消す〉、`--from <binary>` は同じ版の
  バイナリを写すだけ）。確認: `CELERIS_STATE_DIR=<scratchpad>/state scripts/scratch/setup-sccache.sh --from ~/.cargo/bin/sccache` →
  `ok: sccache 0.18.0`、`--from /bin/true` → exit 1（版の不一致）。本番の `~/.local/celeris/tools` には入れていない（人が行う）。
- `deploy/systemd/celeris-sccache.service`（前景の server: `SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1`、env は
  `celerisctl scratch env --server`）と `scripts/selfdeploy/install-units.sh` の対象に追加（置くだけ。有効化は人）。
  `systemctl` は使わずに ExecStart の shell を模して確認: port 4239 で LISTEN → `scratch status` が `(ready)` と
  `hits 0 / misses 0`、`scratch env` に sccache 系 → server を止めると `(unavailable: no sccache server on 127.0.0.1:4239 …)` と
  sccache 系が消える。前景起動（`SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 sccache`）は 0.18.0 で fork しないことを確認（ADR の未確認事項）。
- 手順書 `docs/ops/sccache-l1.md`（導入・有効化・確認・注意）。
- `crates/task-worker/tests/scratch_sccache_e2e.rs`（`#[ignore]`、手動）:
  `CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache cargo test -p task-worker --test scratch_sccache_e2e -- --ignored --nocapture` →
  `wrapper a: rust hits 0 misses 3 (560 ms)` / `wrapper b: rust hits 3 misses 0 (150 ms)` / `plain c: 0 / 3` / `plain d: 0 / 3`、
  `test result: ok. 1 passed`（`cargo_env_with` の wrapper なら別 owner の target で依存も crate も hit、素の sccache は 0）。

### G2 checkpoint 4: 受け入れ条件 3（target の容量）と U3（壁時計）・U6（clippy）（完了 2026-09-28）

`cargo test --workspace --no-run`（`cargo test --workspace` が作る target と同じ。テストの実行時間は含めない）を、空の target から
1 回（clean）と、`crates/task-dispatch/src/dispatcher.rs` の末尾に 1 行のコメントを足した後にもう 1 回（edit、編集ループの代表）。
target の容量は `du -sb`。sccache なしの 3 通り（owner `agent-g2-{a,b,c}`）と、sccache（wrapper 経由、測定用 server）ありの 2 回。

| 構成 | target の容量 | うち `debug/incremental` | clean の壁時計 | edit 後の壁時計 |
|---|---|---|---|---|
| before: cargo の既定（incremental、`debug = true`） | **19.48 GiB**（20,917,220,011 B） | 4.4 G | 53.5 s | **5.5 s** |
| `CARGO_INCREMENTAL=0` だけ | 14.26 GiB | 0 | 43.2 s | 13.0 s |
| **after: `CARGO_INCREMENTAL=0` + `CARGO_PROFILE_DEV_DEBUG=line-tables-only`（Celeris の既定）** | **6.47 GiB**（6,942,244,190 B、**−67 %**） | 0 | **39.3 s** | **11.2 s** |
| after + sccache（L1 に同じ flags の entry が無い 1 回目） | 6.47 GiB | 0 | 43.6 s（Rust 0 / 197 hit、C/C++・asm 382 hit） | 13.4 s |
| after + sccache（別 owner の空の target、L1 が温まった 2 回目） | 6.47 GiB | 0 | 41.0 s（Rust **168 / 29 hit、85.3 %**） | 13.3 s |

- **受け入れ条件 3**: target の容量は 19.48 GiB → 6.47 GiB（−13.0 GiB）。内訳は incremental の廃止で −5.2 GiB、`line-tables-only` で
  さらに −7.8 GiB。`test` profile は `dev` を継ぐので `CARGO_PROFILE_DEV_DEBUG` がテストの実行ファイルにも効く（ADR の未確認事項 → 確認）。
- **U3（壁時計）**: 空からのビルドは速くなった（53.5 s → 39.3 s。incremental の書き出しと debug info が減る）。1 行の編集の後の
  再ビルドは 5.5 s → 11.2 s（**約 2 倍、+5.7 s**。`task-dispatch` の 3 万行を丸ごと作り直す）。Celeris の run は新しい worktree の
  空の target から始まることが多いので既定は `incremental = false` のままにする。長い編集ループの実装エージェントは `unset CARGO_INCREMENTAL`
  してよい（定型文に書いた）。
- **sccache の壁時計**: このリポジトリでは依存の hit（Rust 85 %）でも clean は 39.3 s → 41.0 s とほぼ変わらない（テストの実行ファイルの
  リンク・proc-macro・workspace のメンバーが律速で、依存のコンパイルは 24 コアで並列に隠れる）。sccache の得は主に CPU 時間と、
  並列の run が同時に依存を作り直すときの負荷（未測定）。
- **U6（clippy）**: `cargo clippy --workspace --all-targets -- -D warnings` を wrapper 経由の sccache で 2 つの owner の空の target から →
  どちらも exit 0、Rust 188 hit（依存の `--emit=metadata`）、`Non-cacheable calls 135`（clippy-driver を通る workspace のメンバー）、
  壁時計 21.0 s / 21.1 s。**clippy と `RUSTC_WRAPPER` の組み合わせは壊れない**（メンバーはキャッシュされない。依存だけ hit）。
- 測定の途中で、`CargoTargetPlan::Scratch` の `ScratchSettings` が大きくなって clippy の `large_enum_variant` に当たったので `Box` にした。

### Phase G2 の全体ゲート（2026-09-28、完了）

- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace --no-fail-fast` → exit 0、passed 2550 / failed 0 / ignored 6（G1 の 2543 / 5 から +7 / +1〈手動の `scratch_sccache_e2e`〉）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、警告 0
- `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → ok、再生成後の差分ゼロ
  （`ScratchStatus.sccache` の schema と生成型は checkpoint 2 で commit 済み）
- `cd gui && corepack pnpm@11.27.0 gen:types`（差分ゼロ）`&& typecheck && test` → exit 0、Test Files 73 / Tests 1116 passed
- `bash -n`（`scripts/selfdeploy/*.sh` と `scripts/scratch/*.sh` の 11 ファイルを 1 つずつ）→ 構文エラー 0。`scripts/selfdeploy/tests/*.sh` 3 本 → ok
- 手動: `CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache cargo test -p task-worker --test scratch_sccache_e2e -- --ignored --nocapture` → 1 passed（checkpoint 3）

### 受け入れ条件（ADR-0075 §5 G2）との対応

1. U1 の実測 → checkpoint 1（素の sccache は別 owner で Rust 0 / 192、`CARGO_TARGET_DIR` を外す wrapper で 163 / 192、別パスでも 162 / 192）。
   `scratch_sccache_e2e` が同じ現象を小さな crate で再現する。
2. 全経路の env と `celerisctl scratch env` の一致、server・バイナリが無い／`enabled = false` なら sccache 系なし →
   `sccache_env_is_complete_and_stable`、`sccache_env_is_omitted_without_binary_or_server`、`runs_get_sccache_env_when_the_server_is_up`、
   `runs_fall_back_to_plain_cargo_when_the_server_is_down`、`env_includes_sccache_when_the_server_is_up`、`env_matches_the_dispatcher_env`、
   `every_cargo_path_uses_the_scratch_target_dir`（WU の run と checks・統合の検査も同じ `cargo_env`）、`scratch_cargo_defaults_disable_incremental`。
3. target の容量 19.48 GiB → 6.47 GiB、壁時計（clean 53.5 s → 39.3 s、1 行の編集後 5.5 s → 11.2 s）→ checkpoint 4。

ADR-0075 に「Phase G2 実装時の逸脱・明確化」1〜13 と、D7 に「設定の新しい節は昇格後にだけ本番 config に足す」を追記した。

### 未解決事項・人への依頼

- **有効化（人）**: G2 を含む release の昇格後に `scripts/scratch/setup-sccache.sh --from ~/.cargo/bin/sccache`（または引数なしで
  `cargo install`）→ `scripts/selfdeploy/install-units.sh` → `systemctl --user enable --now celeris-sccache.service` →
  `celerisctl scratch status` で `(ready)` を確かめる（`docs/ops/sccache-l1.md`）。**`[scratch.sccache]` / `[scratch.cargo]` は本番 config に
  足さない**（既定で動く。足すなら昇格後）。
- 残る miss（約 29 / 192）の内訳（workspace のメンバーと `OUT_DIR` 依存の crate）は未確認。G3 の hit 率の測定で見る。
- sccache の得はこのリポジトリの壁時計ではほぼ出ない（CPU 時間と並列 run の負荷は未測定）。G3 の L2 の価値は「新しい worktree で依存を
  作り直す CPU」と「L1 を失った後の復帰」なので、G3 の測定では壁時計に加えて CPU 時間（`/usr/bin/time` の user+sys）も取る提案。
- server が「確かめた直後に落ちた」run の中では sccache の client が server を起こしうる（逸脱 6）。頻度が問題になれば、wrapper の中で
  port を確かめて落ちていれば本物の rustc を直接 exec する案（別 Phase）。

### 提案

- P-G2-1: release.sh のゲートでも `celerisctl scratch env --owner release-<sha12>` の sccache 系を使う（今は `CARGO_TARGET_DIR` だけを
  lease から取る。`[scratch.cargo]` の容量の得と依存の hit を release ゲートにも）。G1 の P-G1-1（N-1 の config の parse 確認）と一緒に。

## G2 の本番反映と sccache L1 の有効化（2026-09-28 05:39Z）

- release `89854b08d3b1`（G2: sccache L1 の配線、`<scratch>/bin/sccache` ラッパー、`CARGO_INCREMENTAL=0` + `line-tables-only`、`celeris-sccache.service` の雛形）。ゲート全通過、verify 全 true（N-1 も ok。config に新セクションを足していないため）。dogfood 4 回目が走行中のままライブ切替（Phase 116 の drain で旧デーモンが run を持ち続ける）。
- 人の手順を Fable が実行: `scripts/scratch/setup-sccache.sh --from ~/.cargo/bin/sccache`（`~/.local/celeris/tools/sccache/bin/sccache` 0.18.0）→ `scripts/selfdeploy/install-units.sh`（`celeris-sccache.service` を配置）→ `systemctl --user enable --now celeris-sccache.service` → `celerisctl scratch status` に `sccache L1 … (ready) · port 4236`。以後の run は `RUSTC_WRAPPER` 経由で L1 を使う。
- 発見 P-G2-2: release の bundle（`~/.local/celeris/releases/<sha>/scripts/`）には `scripts/selfdeploy/` しか入らず、`scripts/scratch/` と `deploy/systemd/` が無い（release からは setup-sccache.sh / install-units.sh の unit 配置が動かない）。今回は作業チェックアウトから実行。release.sh の bundle に両方を含めるべき。
- scratch pool の実物: `targets 28.9 GB / 100 GB`（走行中の WU の target 21 GB が p0、release のゲートの target 7.9 GB が seed）。legacy 2 件は 1 時間更新なしで回収予定。
## Phase G3（L2: webdav の階層 cache server + 非同期 flusher + L2 の GC + 監視）— 着手 2026-09-28

作業は git worktree の中（main へ merge / push しない）。自分のビルドは
`CARGO_TARGET_DIR=/var/lib/celeris/scratch/targets/agent-g3/target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only`。

### G3 checkpoint 1: U5 の実測と設計の確定（完了 2026-09-28）

測定の条件: sccache 0.18.0（`~/.cargo/bin/sccache`）、専用の sccache server（`SCCACHE_SERVER_PORT=4292`、
`SCCACHE_WEBDAV_ENDPOINT=http://127.0.0.1:4291`、`SCCACHE_WEBDAV_KEY_PREFIX=sccache`、`SCCACHE_WEBDAV_TOKEN=u5-secret-token`）、
記録用の stub（`scripts/scratch/u5/webdav-record-stub.py`。loopback の 4291、メソッド・パス・応答・長さ・`Authorization` を 1 行ずつ記録）。
ビルドは 2 crate（lib + bin）の小さな workspace を `CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache cargo build --offline`（target は
`/var/lib/celeris/scratch/targets/agent-g3-u5/target`、各回 空から）。再現: `scripts/scratch/u5/measure.sh <case>`。

1. **発行されるメソッドとパス**（case `ok`、stub が PROPFIND に正しく答えるとき）:

   ```
   GET      /sccache/.sccache_check                -> 404          # server 起動時の storage check（読み）
   PROPFIND /sccache/            (Depth: 0)        -> 207          # 親の collection があるか
   PUT      /sccache/.sccache_check  (13 bytes)    -> 201          # storage check（書き。中身 "Hello, World!"）
   GET      /sccache/9/8/8/988fc487…c95a7          -> 404          # 1 回目のビルド: miss
   PROPFIND /sccache/9/8/8/      (Depth: 0)        -> 207          # PUT の前に毎回、親の collection を確かめる
   PUT      /sccache/9/8/8/988fc487…c95a7 (10868 bytes) -> 201
   GET      /sccache/9/8/8/988fc487…c95a7          -> 200 (10868) # 2 回目のビルド: hit
   ```

   - path は `/<KEY_PREFIX>/<k0>/<k1>/<k2>/<key>`（sccache の `normalize_key`）。key は 64 桁の 16 進（sha256）。
     `SCCACHE_WEBDAV_TOKEN` は `Authorization: Bearer <token>` で毎回送られる（0.18 の config は `USERNAME` / `PASSWORD` / `TOKEN` を読む）。
   - HEAD・DELETE・MOVE・COPY・PROPPATCH・Range 付きの GET は出なかった。
   - **PROPFIND が 404 なら opendal は親から順に PROPFIND して MKCOL する**（stub の初版: `PROPFIND /sccache/9/8/8/` `/sccache/9/8/`
     `/sccache/9/` → 404 のあと `/sccache/` の 207 で止まった）。**207 の応答に `getlastmodified` が無いと opendal は PUT せずに
     失敗する**（`write close failed … propfind response missing getlastmodified`、sccache の stats は `cache_write_errors 1`）。
     → cache server は **collection（`/` で終わる path）の PROPFIND に常に 207 + `resourcetype collection` + `getlastmodified`** を返す
     （ディレクトリは仮想。MKCOL は実装するが 201 を返すだけで、通常は呼ばれない）。
   - stats: 1 回目 `misses Rust 1 / writes 1`、2 回目 `hits Rust 1`。entry の中身は ZIP（各 member は sccache が zstd で圧縮済み。
     `cache_io.rs`）。L2 の zstd は容量の得ではなく checksum のため（ADR の見込みどおり）。
2. **backend が起動時に居ない**（case `down-at-start`）: `sccache: error: Server startup failed: cache storage failed to read: …
   Connection refused`、exit 2。**sccache の server が起動しない**。storage check の GET が 404 以外（case `500`: 500 を返す）でも同じく
   起動失敗。→ `celerisctl scratch env --server` は cache server の `/healthz` を確かめ、応答が無ければ webdav を与えず G2 の
   local disk（`SCCACHE_DIR`）に戻す（さもないと `celeris-sccache.service` が restart を繰り返す）。
3. **backend が起動後に死ぬ**（case `down-mid`: storage check の後に stub を止める）: ビルドは **exit 0**。stats は
   `misses Rust 1 / writes 0 / write_err 1`（GET の失敗は miss、PUT の失敗は write error。compile は失敗しない）。
4. **backend が遅い**（case `slow`: GET と PUT に 8 秒の sleep）: ビルドは exit 0 だが **16.4 秒**（通常 1 秒未満）。sccache は
   GET の応答を待ってから compile し、**PUT の完了も待ってから結果を返す**（stub の PUT の応答時刻 = ビルドの終了時刻）。0.18 には
   backend の timeout が無い（8 秒では切れなかった。`cache_timeouts 0`）。→ (a) PUT は L1 に書いた時点で応答する（L2 を critical path
   に入れない。入力メモの原則が必須であることの実測）、(b) GET の L2 は `get_timeout_ms`（既定 500）で打ち切って miss を返す、
   (c) cache server が hang した場合に備え、dispatcher は run の開始時に `/healthz`（300 ms）を見て、sccache が webdav で動いているのに
   応答が無ければ `RUSTC_WRAPPER` を与えない（素の cargo）。
5. **sccache 0.18 は multilevel cache（disk + remote の階層）を内蔵**している（`src/cache/multilevel.rs`）。本 Phase は ADR の決定
   （D5 (b): 階層は Celeris の cache server が持ち、sccache からは 1 つの webdav に見える）のまま進める。理由: multilevel の L2 への
   書き込みが critical path の外かは未確認で、帯域制限・L2 の切り離し・GC を Celeris が制御できない。

**確定した設計**（ADR-0075「Phase G3 実装時の逸脱・明確化」に同じ内容）:

- 実装するメソッド: GET / HEAD / PUT / PROPFIND（Depth 0 のみ。collection は常に 207、ファイルは L1 か L2 にあれば 207）/ MKCOL（201）。
  それ以外は 405。key = path の最後の要素。`.sccache_check` は L2 に流さずメモリだけで扱う。`/healthz` と `/stats` は認証なし、DAV は
  token（`Authorization: Bearer`、または Basic の password）。
- cache server の L1 は `<scratch>/cache-l1/`（G2 の sccache の disk cache `<scratch>/sccache-l1/` と分ける。cache server が落ちたときの
  fallback で sccache が同じ dir を LRU 走査して cache server の entry を消さないため）。
- `celerisctl scratch env --server`: cache server が健康なら `SCCACHE_WEBDAV_ENDPOINT` / `SCCACHE_WEBDAV_KEY_PREFIX` / `SCCACHE_WEBDAV_TOKEN`、
  でなければ G2 の `SCCACHE_DIR` / `SCCACHE_CACHE_SIZE`。選んだ方式を `<scratch>/bin/sccache-server.mode` に書き、dispatcher はそれが
  webdav のときだけ `/healthz` も確かめる。client（run）の env は G2 のまま（webdav 系を入れない: 万一 client が server を起こしても
  local disk で起動し、token を run の env に出さない）。

### G3 checkpoint 2: `crates/scratch-cache` の中核（TieredStore・flusher・L2 の GC）+ 単体テスト（完了 2026-09-28）

- 新規 crate `crates/scratch-cache`（`bucket` = token bucket、`clock` = 時計〈テストは仮想〉、`key`、`l2` = `L2Backend` / `DirL2` /
  上限付きの I/O スレッド `L2Exec` / 切り離しの `Breaker`、`store` = `TieredStore`、`server` = WebDAV のサブセット〈checkpoint 3〉）。
- L1 `<l1>/<k0k1>/<key>`（tmp → rename、索引はメモリ、起動時に 1 回走査、hit で mtime を 1 時間に 1 回 touch）。L1 の上限
  （`l1_max × 0.9` を超えたら `× 0.7` まで LRU）は**未 flush の entry を落とさない**。
- L2 `<l2>/<k0k1>/<key>.zst`（zstd level 3・content checksum 付き、tmp `.<key>.zst.tmp-<pid>-<nanos>-<seq>` → write → fsync →
  rename、既にあれば書かない）。L2 の root が見えない（NFS が外れた・dir を消された）ときは root を作り直さずエラー
  （mount point の下のローカルに書かないため）。
- GET の L2 は `L2Exec`（4 スレッド、待ち行列 16）で `l2_get_timeout`（500 ms）まで待ち、失敗・タイムアウト・満杯は miss。3 回連続で
  切り離し（5 s → 最大 300 s の指数バックオフ）、明けたら 1 回試して成功で復帰。壊れた `.zst` は miss にして消す（切り離しの理由にしない）。
  L2 hit で mtime を 1 日 1 回まで touch（L2 の GC の LRU）。
- flusher（1 スレッド）: 待ち行列の先頭を L1 から読み、L2 に既にあれば書かず、zstd で包んで token bucket（既定 25 MB/s、burst 25 MB）
  の不足分を待ってから書く。失敗は先頭に戻して breaker に数える。待ち行列の上限（4096 MB）を超えたら古いものから「L2 に書かない」で
  落とす（L1 の LRU の対象に戻す）。未 flush の key は `<l1>/.pending`（`<key>` = 積んだ / `-<key>` = 済んだ）に追記し、再起動後に
  積み直す（空になったら切り詰め）。
- L2 の GC: 走査して `l2_max_bytes` を超えていれば mtime の古い順に `× 0.9` まで消す（1 日 1 回、起動 60 s 後に初回。走査で古い tmp も片づける）。
- stats の型 `task_ops::daemon::ScratchCacheStats`（`celeris.scratch-cache-stats/1`）と `ScratchStatus.cache`（`ScratchCacheView`、
  `#[serde(default)]`）。schema と GUI の生成型を再生成（checkpoint 4 で表示に使う）。
- テスト: `cargo test -p scratch-cache` → **13 passed / 0 failed**（3 回繰り返して同じ、1.5 s）:
  `get_prefers_l1_then_l2_then_miss`、`l2_hit_is_promoted_to_l1`、`put_returns_before_the_l2_write`（L2 の書き込みが 1.5 s 遅い
  `SlowDirL2` で PUT が 500 ms 未満に返り、その時点で L2 に無く、後で flusher が書く）、`flusher_respects_the_token_bucket`（仮想の
  時計で 40 × 2.5 MB の非圧縮データ〈≈100 MB〉を flush: burst を除いた帯域が **25.00 MB/s 以下かつ 22.5 以上**）、
  `token_bucket_limits_bytes_over_any_interval`、`l2_write_is_atomic_and_idempotent`（8 スレッドが同じ key を同時に書いても中身が
  正しく tmp が残らない、2 回目は書かない）、`corrupt_l2_entry_is_discarded`（1 byte 反転と末尾 7 byte の切り詰めが miss になり消える）、
  `l2_unavailable_degrades_to_l1_only`（dir の rename・権限 000・2 s 遅い L2〈100 ms で打ち切り〉のそれぞれで L1 は応答し続け、
  3 回で degraded、バックオフ後に復帰）、`l2_gc_evicts_lru`（10 個中 hit で touch した最古の 1 個を残して古い 5 個を消す）、
  `l1_eviction_keeps_unflushed_entries`、`pending_log_is_replayed_after_restart`、`invalid_keys_are_rejected`、`l1_only_store_never_queues`。
- `cargo clippy --workspace --all-targets -- -D warnings` → 警告 0。

### G3 checkpoint 3: WebDAV server・`celeris cache-server`・unit の雛形・`scratch env --server` の切替とフォールバック（完了 2026-09-28）

- `scratch_cache::server`（axum の fallback 1 つで method を見る）: `GET` / `HEAD` / `PUT` / `PROPFIND`（collection は常に 207 +
  `getlastmodified`、ファイルは在れば 207）/ `MKCOL`（201）、それ以外 405。key = path の最後の要素（不正は 400）、`.sccache_check` は
  メモリ。`Authorization: Bearer <token>`（違えば 401）、`/healthz`・`/stats` は認証なし。本文の上限 1 GiB。
- `celeris cache-server`（`crates/celeris/src/cache_server.rs`、`celeris --config <path> cache-server`）: scratch が無効なら起動しない
  （L1 を NFS に置かない）、token を `ensure_token`（既定 `<scratch>/cache-server.token`、0600、初回に生成）、`127.0.0.1:<port>` に bind、
  SIGTERM で止まり未 flush は `.pending` に残す。
- 設定: `[scratch.l2]`（`enabled` / `dir` 既定 `$CELERIS_STATE_DIR/cache/sccache-l2` / `max_gb` 300 / `flush_mbps` 25 /
  `flush_queue_max_mb` 4096 / `get_timeout_ms` 500 / `io_threads` 4 / `gc_interval_secs` 86400）と `[scratch.cache_server]`
  （`enabled` / `port` 4237 / `token_file`）。どちらも書かなくても動く（D7 の N-1 の規則）。
- `celerisctl scratch env --server`: cache server の `/healthz` を最大 5 回（200 ms 間隔）見て、応答すれば webdav
  （`SCCACHE_WEBDAV_ENDPOINT` / `SCCACHE_WEBDAV_KEY_PREFIX=sccache` / `SCCACHE_WEBDAV_TOKEN` / `SCCACHE_SERVER_PORT` /
  `SCCACHE_IDLE_TIMEOUT`、`SCCACHE_DIR` なし）、応答しなければ G2 の local disk。先頭に `# celeris: sccache backend = …` の 1 行
  （journal に残る）。選んだ方を `<scratch>/bin/sccache-server.mode` に書く。
- dispatcher / `scratch env`（client）: `resolve_sccache` が、mode が webdav のときだけ cache server の `/healthz`（300 ms）も見て、
  応答が無ければ `RUSTC_WRAPPER` を与えない（U5 の「backend が遅いと sccache が待ち続ける」への備え）。client の env は G2 のまま。
- unit の雛形 `deploy/systemd/celeris-scratch-cache.service`（`celeris … cache-server`、`Before=celeris-sccache.service`）と
  `install-units.sh` の対象に追加（置くだけ）。`celeris-sccache.service` に `After=celeris-scratch-cache.service`（引き込まない）。
- テスト: `scratch_cache` の統合テスト `tests/webdav.rs` 3 件（`sccache_request_sequence_round_trips` = U5 の順序をなぞる、
  `auth_and_invalid_requests_are_rejected`、`l2_hit_through_http_is_promoted`）→ 3 passed。
  `task_dispatch::dispatcher::tests::cache_server_down_means_no_rustc_wrapper`（webdav + cache server なし → 素の cargo、webdav +
  応答あり → 与える、disk → cache server が無くても与える）、`celerisctl::commands::scratch::tests::env_server_switches_to_webdav_when_the_cache_server_is_up`、
  `task_worker::scratch::tests::cache_server_health_reads_a_minimal_http_response`、`celeris::config::tests::scratch_l2_defaults_work_without_the_section`
  → いずれも passed。偽の server は要求の空行まで読み切ってから答える（高負荷で要求が分かれて届いても RST にしない）。
- 手動 e2e（`#[ignore]`）: `CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache CELERIS_E2E_DIR=/var/lib/celeris/scratch/targets/agent-g3-e2e
  cargo test -p scratch-cache --test sccache_webdav_e2e -- --ignored --nocapture` → 1 passed:

  ```
  A (cold): 340.68 ms rust hits 0 misses 3 · server puts 3 gets 3 misses 3
     flushed to L2: written 3 (23886 bytes) at 25 MB/s cap
  B (L1): 168.79 ms rust hits 3 misses 0 · server l1_hits 3 l2_hits 0
  C (L2 → promote): 172.80 ms rust hits 3 misses 0 · server l2_hits 3 promotes 3
  D (cache server down): ok=true 208.20 ms rust hits 0 misses 3
  ```

  （最初の実行は B が 0 / 3: wrapper を通さず `RUSTC_WRAPPER=sccache` にしたため、owner ごとの `CARGO_TARGET_DIR` が key に入った。
  G2 の U1 と同じ。e2e は `<scratch>/bin/sccache` と同じ wrapper を使うように直した。）

### G3 checkpoint 4: 監視（status / API / GUI）、schema と gen:types（完了 2026-09-28）

- `ScratchStatus.cache`（`ScratchCacheView`: state `ready` / `disabled` / `unavailable`、reason、endpoint、sccache_mode〈`webdav` / `disk`〉、
  stats = cache server の `/stats`〈`ScratchCacheStats`〉）。`task_dispatch::scratch_gc::{cache_view, query_cache_stats}`（loopback の
  `/stats`、500 ms。`[scratch.cache_server] enabled = false` なら問い合わせない）。
- daemon の tick（`DaemonSnapshot.scratch.cache`）→ `GET /api/v1/metrics/scratch` に同じ欄。`celerisctl scratch status` に 5〜6 行
  （cache server の状態と sccache の backend、gets と L1 / L2 hit・miss の率・promotes・puts、L1 の使用量、L2 の状態・使用量・
  errors / timeouts / corrupt、切り離し中なら理由と再試行の時刻、flush の待ち行列・最古の待ち時間・最終 flush・書いた量・帯域の上限、L2 GC）。
- GUI のデーモン画面の 1 行: 「scratch 62 GB / 100 GB（pinned 18 GB、実効上限 150 GB） · L1 hit 71% · L2 hit 12% · flush 遅延 3 s」。
  L2 の切り離し（degraded）は注意色、cache server が応答しなければ「· cache server unavailable」。
- テスト: `celerisctl::commands::scratch::tests::status_reads_the_cache_server_stats`（本物の `scratch_cache` の server を loopback の
  port 0 に立てて `status_of` → `ready`、puts / gets / l1_hits / flush_written / l2_state、居なければ `unavailable`）、GUI
  `scratchLine` の G3 のケース（`gui/test/unit/daemon.test.ts`）。
- schema / 生成型: `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated`
  → ok、`cd gui && corepack pnpm@11.27.0 gen:types && typecheck && test` → Test Files 73 / Tests 1117 passed、lint 0 error。
- **手動 e2e（本物の `celeris cache-server` + `celerisctl scratch env --server` + sccache 0.18 でこのリポジトリの `cargo build --workspace`）**。
  config・scratch・L2 は `/var/lib/celeris/scratch/targets/agent-g3-srv/` の下（L2 もローカル。NFS の遅延は測っていない）、cache server
  4293、sccache 4294（`celeris-sccache.service` と同じ `SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1`、env は `env --server`）。

  | 回 | 状態 | 壁時計 | CPU user / sys | sccache Rust hit / miss | cache server |
  |---|---|---|---|---|---|
  | env --server（cache server なし） | — | — | — | — | `# celeris: sccache backend = disk …（no cache server on 127.0.0.1:4293 …）`、`SCCACHE_DIR` |
  | env --server（あり） | — | — | — | — | `backend = webdav http://127.0.0.1:4293`、`SCCACHE_WEBDAV_*`、mode = `webdav` |
  | A | 空（cold） | 44.3 s | 15.5 / 9.5 s | 0 / 196 | puts 614、misses 616、**flush 612 件 215.8 MB、ビルドの終了時に待ち行列は空**（25 MB/s で ≈ 8.6 s 分） |
  | B | cache server を止め L1 を消して起こし直す（L2 だけ） | 37.8 s | 15.1 / 9.5 s | **165 / 196** | **l2_hits 582、promotes 582**、misses 33、puts 31（104.9 MB） |
  | C | 別 owner（L1） | 37.8 s | 15.0 / 9.4 s | 166 / 196 | l1_hits 585、puts 30 |

  - `scratch status`: `cache server http://127.0.0.1:4293 (ready) · sccache backend webdav` / `gets 1234 · L1 hit 47.6% · L2 hit 47.2% ·
    miss 5.3% · promotes 582 · puts 61` / `L2 … (ok) 401.0 MB / 300.0 GB (673 entries …)` / `flush queue 0 … · written 61 (195.2 MB) · cap 25 MB/s`。
  - cache server を止めると `scratch status` の sccache 行が `unavailable: sccache uses the webdav backend but the cache server on
    127.0.0.1:4293 does not answer /healthz`（dispatcher と `scratch env` は `RUSTC_WRAPPER` を与えない）、cache server 行は `unavailable`。
  - **所見**: 毎回 miss する約 30 crate（workspace のメンバーと `OUT_DIR` 依存。G2 の残り）は owner ごとに key が変わるので、**毎 run 約
    100 MB を L2 に書き、二度と hit しない**。L2 の LRU（300 GB）で回収されるが NFS の帯域を使う（提案 P-G3-2）。
  - 壁時計の得は小さい（44.3 → 37.8 s。リンクと workspace のメンバーが律速、G2 の U3 と同じ）。L2 の価値は「L1 を失った後・別マシン
    でも依存を作り直さない」（B が C と同じ hit 率）。

### Phase G3 の全体ゲート（2026-09-28、完了）

- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace --no-fail-fast` → exit 0、passed 2571 / failed 0 / ignored 7（G2 の 2550 / 6 から +21 / +1〈手動の `sccache_webdav_e2e`〉）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、警告 0
- `UPDATE_SCHEMA=1 cargo test -p task-core schema && UPDATE_SCHEMA=1 cargo test --workspace committed_schema_matches_generated` → ok、再生成後の差分ゼロ
- `cd gui && corepack pnpm@11.27.0 gen:types`（差分ゼロ）`&& typecheck && test` → exit 0、Test Files 73 / Tests 1117 passed。lint 0 error
- `bash -n`（`scripts/selfdeploy/*.sh`・`scripts/scratch/*.sh`・`scripts/scratch/u5/*.sh` の 12 ファイル）→ 構文エラー 0。`scripts/selfdeploy/tests/*.sh` 3 本 → ok
- 手動: `CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache CELERIS_E2E_DIR=/var/lib/celeris/scratch/targets/agent-g3-e2e cargo test -p scratch-cache
  --test sccache_webdav_e2e -- --ignored --nocapture` → 1 passed（checkpoint 3）。本物の `celeris cache-server` でこのリポジトリ（checkpoint 4）。

### 受け入れ条件（ADR-0075 §5 G3）との対応

1. U5 の記録 → checkpoint 1（メソッドとパス、PROPFIND の `getlastmodified`、起動時の storage check の失敗、起動後の停止でコンパイル継続、
   遅い backend を待ち続ける）。実装したメソッドは記録した集合 + HEAD。
2. GET は L1 → L2 → 404、L2 hit は promote、PUT は L1 で即応答、flusher は 25 MB/s を超えない、tmp → fsync → rename、同時書き込みで壊れない →
   `get_prefers_l1_then_l2_then_miss`、`l2_hit_is_promoted_to_l1`、`put_returns_before_the_l2_write`、`flusher_respects_the_token_bucket`、
   `token_bucket_limits_bytes_over_any_interval`、`l2_write_is_atomic_and_idempotent`、`sccache_request_sequence_round_trips`、`l2_hit_through_http_is_promoted`。
3. L2 が読めない（dir を消す・権限 000・遅い L2）→ L1 だけで応答、連続失敗で切り離し、バックオフ後に復帰。壊れた `.zst` は miss で消す →
   `l2_unavailable_degrades_to_l1_only`、`corrupt_l2_entry_is_discarded`。
4. L1 の上限で LRU、未 flush は落とさない、再起動後に `.pending` から積み直す → `l1_eviction_keeps_unflushed_entries`、`pending_log_is_replayed_after_restart`。
   L2 の GC → `l2_gc_evicts_lru`。
5. cache server が止まっているとき run は素の cargo で成功（webdav のとき）→ `cache_server_down_means_no_rustc_wrapper`、
   `env_server_switches_to_webdav_when_the_cache_server_is_up`。sccache 自身の挙動は U5（down-mid: exit 0）と e2e の D（cache server 停止中の
   ビルド ok）。
6. `scratch status` / metrics / GUI に L1 / L2 の hit 率・サイズ・flush の遅延 → `status_reads_the_cache_server_stats`、GUI `scratchLine`、
   手動 e2e の `scratch status` の出力（checkpoint 4）。

ADR-0075 に「Phase G3 実装時の逸脱・明確化」1〜17 を追記し、状態の行を更新した。`docs/ops/sccache-l1.md` に「L2」の節（有効化の手順・確認・注意）。

### 未解決事項・人への依頼

- **有効化（人）**: G3 を含む release の昇格後に `scripts/selfdeploy/install-units.sh` → `systemctl --user enable --now
  celeris-scratch-cache.service` → `systemctl --user restart celeris-sccache.service` → `celerisctl scratch status` で
  `cache server … (ready) · sccache backend webdav` を確かめる（`docs/ops/sccache-l1.md` の「L2」）。**`[scratch.l2]` /
  `[scratch.cache_server]` は本番 config に足さない**（既定で動く。足すなら昇格後）。
- **NFS 上の L2 の測定（U4）**: 本番の L2（`~/.local/celeris/cache/sccache-l2`、TrueNAS の NFS、1GbE）で、L1 を消した後の再ビルドの壁時計と
  `l2_timeouts`（GET の L2 は 500 ms で打ち切る）を見る。頭打ちなら事前 promote（D5 の最後）を別 Phase で検討。
- L2 に NFS の root が無い状態（mount が外れた）で cache server を起動すると、初回の `create_dir_all` が mount point の下のローカルに
  root を作りうる（`TieredStore::open` の初回だけ。以後の書き込みは root を作らない）。`/home` が NFS そのものなので現状は起きないが、
  L2 を別の mount に移すなら起動時の NFS 検査（D1 の `is_on_nfs` を L2 に使う）を足す。

### 提案

- P-G3-1: `celeris-sccache.service` を cache server の再起動に追従させる（`PartOf=celeris-scratch-cache.service` か、cache server が
  `/healthz` を返し始めた後に `systemctl --user try-restart celeris-sccache.service`）。今は人が再起動する。
- P-G3-2: 毎 run 約 100 MB の再利用されない L2 書き込み（owner ごとに key が変わる約 30 crate）を減らす。案: (a) `OUT_DIR` 依存の key を
  owner に依らなくする（build script の出力先の path を wrapper で正規化できるかの調査）、(b) L2 への admission を「L1 で 1 回以上 hit した
  entry だけ」にする（初回の run の entry は L2 に行かず、2 回目から効く）。

## 障害調査: クラスタ画面の pegasus TOTP で 503（2026-09-28）

- 事実: `POST /clusters/pegasus/connect` は 30 秒待って 502 `timed out waiting for ssh`。GUI の API クライアントの timeout は 15 秒（`client.server.ts` の `DEFAULT_TIMEOUT_MS`）なので、GUI 側が先に諦めて「celeris に接続できません」（503）になる。
- 原因: `ssh pegasus` が **publickey で拒否**される（`Permission denied (publickey)`。pegasus03 は `Authentications that can continue: publickey` しか返さず、TOTP の keyboard-interactive 段階に進まない）。同じ鍵（`~/.ssh/id_ed25519`、SHA256:vzR0g5GM…）で sirius は `Server accepts key` → keyboard-interactive に進むので、手元の鍵・設定（NFS 移行後の `~/.ssh`、ControlPath の変更）は問題なし。pegasus 側で公開鍵が外れている（authorized_keys / アカウント状態）と考えられる。人がログインノードで鍵を登録し直す必要がある。
- 副次: sirius に 5 時間 pending のままの接続試行（ssh master の scope）が残っていたので `DELETE /clusters/sirius/connect` で取り消した。pegasus の失敗した scope（`celeris-ssh-master-pegasus-BJ8B8QND.scope`、failed）も残っている。
- 提案 P-G3-3: (a) connect の API は ssh の `Permission denied` / 接続失敗を待たずに即時返す（stderr を監視）、(b) GUI の connect 系だけ timeout を API の待ち（30 秒）より長くするか、API が即座に pending を返して GUI が状態を poll する、(c) 失敗した ssh master の scope を daemon が片付ける。
- 別の異常: ユーザー journal（`journalctl --user`）が 9/26 06:00 以降のエントリを持たない（`_UID=1001` で 0 行、`/var/log/journal` に user-1001 の journal が無い）。デーモンのログが追えない。root で `journalctl -u user@1001.service -n 5` と `systemctl status systemd-journald` を確認する必要がある（人）。
