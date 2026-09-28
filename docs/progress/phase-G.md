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
