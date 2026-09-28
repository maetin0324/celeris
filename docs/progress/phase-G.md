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
