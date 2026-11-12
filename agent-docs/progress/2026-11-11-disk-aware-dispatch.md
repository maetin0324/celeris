---
title: 空きの見込みで dispatch を絞り、空きの下限を filesystem の大きさに比例させる
tasks: [01M75CV7TY7GMYY8PG3BE0SCBJ]
status: done
completed: 2026-11-11
updated: 2026-11-12
---
# 空きの見込みで dispatch を絞り、空きの下限を filesystem の大きさに比例させる

設計: [ADR 2026-11-11-disk-aware-dispatch](../adr/2026-11-11-disk-aware-dispatch.md)。

## 実装前 brief

1. 根本原因: dispatch は今の空きだけを見て（下限 5 GiB 固定）、走っている target がこれから増える量と新しい run の target の量を見ない。release.sh は release-build の target の上限を build の前にしか確かめない。
2. 責務 owner: dispatch の前の判定（`dispatcher/dispatch_run.rs` の起動前の段）と scratch の見積り（`scratch_gc::build_status`）。release-build は `scripts/selfdeploy/lib.sh` の刈り込み。
3. 既存の再利用先: scratch の走査と測定（lease の `size_bytes`、`ScratchOwnerView`）、`fs_stats`、`sd_release_prune_stale_test_binaries`・`sd_release_prune_enforce_limit`。
4. 選択肢: (a) 今の空きの下限を上げるだけ、(b) 見込みで判定する。(a) は走っている target の増分を見ないので (b) を採った。見込みは repo の最近の最大（operator の計測で seed との共有はほぼ 0）。
5. 新しい状態: `ScratchState.admission`（`disk_admission::Gate`。待たせている run のメモリ〈中の Mutex。dispatch の途中で `&self` から呼ぶため〉。file には書かない）。試験用の口 `Gate.probe`（空きと大きさを注入）。config `[dispatch] min_free_disk_pct`・`[scratch] forecast_disk_ratio`。Dispatcher の field・method は増やしていない（health の `size.responsibility-growth`）。
6. 撤去対象: `[dispatch] min_free_disk_mb`（読み込みエラー）、`dispatcher.rs` の `free_disk_mb`、今の空きだけで決める dispatch。
7. 不変条件と試験: `dispatcher/tests/disk_forecast.rs`（待つ・戻れば始まる・続きは増分・0 本なら通す・log 1 回・通知なし）、`disk_admission.rs` の単体試験、`config/tests.rs`（旧い欄の拒否・範囲）、`scripts/selfdeploy/tests/release_target_settled_after_build.sh`。

## 変更

- D1: `crates/task-dispatch/src/disk_admission.rs`（新）。`空き − Σ 走っている target の増分 − 今の pass で通した見込み − この run の見込み ≥ 予備` でなければ run を始めず `ready` のまま待たせる（lease・attempts の前）。見込みは target が pool にある同じ `repo_key` の owner の測った大きさの最大（GC の推定と同じ `task_worker::scratch::repo_max_sizes`）。増分を数えるのは run か review の検査が今書いている owner だけ（`target_written_now`）。続きの run（owner の target がある）は増分だけ。worker run が 0 本なら通す。待ち始めに 1 回だけ `tracing::info!`（`disk forecast: run held`）。通知・受信箱には書かない。呼び出しは `dispatch_run.rs` の write-set の判定の直後（planner・Remote の run は除く）。
- D2: `[dispatch] min_free_disk_mb` を `[dispatch] min_free_disk_pct`（既定 10、範囲 [0, 100)）に置き換えた。下限 = 容量 × pct。`check_disk_space`（`disk_admission::find_low_disk`）・scratch の GC と実効上限（`run_gc` の `min_free_bytes`）・seed の更新の保留・`celerisctl scratch` が同じ下限を使う。`[scratch] forecast_disk_ratio`（既定 1.0、範囲 (0, 1]）を足した。
- D3: release-build の target が上限を超えて残る理由: `sd_release_prune_enforce_limit` は release.sh の build の前にしか呼ばれず、build で育った target（本番 88.0 GB）はそのまま lease に残っていた。adopt の経路は原因ではない（adopt の後も開始時の確認は通る）。release.sh の EXIT trap に `sd_release_target_settle`（この build の開始時刻より古い workspace の成果物の刈り込みと上限の確認）を足した。
- D4: `ScratchStatus.admission`（`ScratchAdmissionView`: 空き・予備・係数・増分の合計・repo の見込み・owner ごとの見込み・待たせている run）。`GET /api/v1/metrics/scratch` と `celerisctl scratch status`（`--json` と表の両方）に出る。待たせている run（`held`）の正本は daemon の `Gate` のメモリだけで、daemon の snapshot が `Gate::with_held` で埋める。`celerisctl scratch status` の `held` は `null`（表は「only the daemon knows」と API を示す）。API schema（`docs/api/v1/api-v1.schema.json`）と web の生成型を作り直した。
- attempt 3: review の不合格 1 件と申し送り 2 件を直した。(1) `docs/ops/local-disk-growth.md` の `targets_max_gb` の上げ幅が旧い 5 GiB の下限で計算した「約 190」のままだった。下限 30 GiB を 1 倍で引いた 270 GiB（`targets_max_gb` は約 179 まで）に直し、旧い 3 倍の余白は D1 の見込みが守ると書いた。(2) 見込みの材料（`samples_from_owners`）が target の消えた lease の記録も数えていた。`has_target` で絞り、repo の最大を GC の推定と同じ関数（`repo_max_sizes`）にした。(3) class `p0` をすべて増分に数え、lease の生きた release-build・まだ始まらない WU も増分を持っていた。分類の理由で絞り（`target_written_now`: `task running`・`task reviewing`・`work unit running`）、WU の理由を `work unit running` と `work unit queued` に分けた。API の説明が変わったので schema と web の生成型を作り直した。
- 文書: ADR-0075 に付記、`docs/ops/local-disk-growth.md`・`docs/ops/build-tmp-hygiene.md`・`docs/architecture-map.md`・`config/celeris.example.toml`。

commit（件名）:

- `phase 1: hold runs whose cargo target would not fit the disk forecast, make the free-space floor proportional, cap release-build after the build`
- `phase 1: keep the dispatcher module from growing (gate behind &self, disk paths label in disk_watch) and break the scratch_gc cycle`（health の `size.responsibility-growth`〈`dispatch_one` と dispatcher module の行〉と `deps.cycle`〈disk_admission ↔ scratch_gc〉を直した。`dispatch_one` の disk critical の path の組み立てを `disk_watch::paths_label` に移し、`find_low_disk` は statvfs の関数を引数で受ける）
- `phase 1: 進捗に検査の結果を書く`
- `phase 2: keep the held runs only in the daemon's memory and drop the leftover min_free_disk_mb references`（attempt 2。review の指摘 2 件を直した。(1) 初版は待たせている run を pool の `admission.json` に書き、daemon の status もそれを読んでいた。verify の daemon は同じ pool で dispatch の pass を回すので、空の一覧で本番の status を上書きした。file を廃止し、daemon の snapshot がメモリから埋める。記録の刈り込みは候補を照合する pass だけで行う。(2) 消した欄 `min_free_disk_mb` が `tests/e2e/tests/account_pool_scenarios.rs` の試験 config に残り、release gate の `nextest --workspace` で celeris が読み込みエラーになっていた。`min_free_disk_pct = 0` に直し、`DispatchConfig.min_free_disk_pct`・`disk_watch.rs`・celerisctl の `scratch_tests.rs`・`dispatcher/tests/build_cache.rs` の説明も直した）
- `phase 2: 進捗に attempt 2 の検査の結果を書く`
- `phase 3: count only present targets for the repo forecast, grow only targets a run writes now, fix the targets_max_gb bound`（attempt 3）

## 証拠

（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `bash scripts/dev/test-affected.sh`: exit 0（attempt 2、1039 秒、開始時 load average 約 40）。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・e2e・llm-proxy・task-api・task-dispatch・task-ops・task-worker と web。nextest 6739 passed・23 skipped、web 104 file・712 passed、repo_health_gate all checks passed。
- `cargo build -p celeris -p celerisctl --bins && cargo nextest run -p e2e --test account_pool_scenarios`: 3 passed（review で落ちた `account_selection_follows_headroom…` を含む。試験 config を `min_free_disk_pct = 0` に直した後）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: verdict=pass、fail 級 0。
- `cargo nextest run -p task-dispatch --lib disk_forecast disk_admission cleanup_and_disk scratch_gc disk_watch`: 33 passed。
- `cargo nextest run -p task-dispatch --lib disk_forecast disk_admission cleanup_and_disk`（attempt 2）: 13 passed（`disk_admission` の `the_daemon_status_carries_its_own_held_runs_from_memory` は、別の Gate〈verify の daemon〉の空の pass が status の `held` を変えないことを確かめる）。
- `bash scripts/selfdeploy/tests/release_target_settled_after_build.sh`・`release_target_delete_retry.sh`・`release_prune_stale_test_binaries.sh`・`release_uses_scratch_lease.sh`・`release_prune_production_scale.sh`・`prune_tests_stay_in_tmp.sh`: すべて exit 0。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated`: 1 passed（schema を作り直した）。

## 本番への移行

- 本番 config（`~/.config/celeris/config.toml`）に `[dispatch] min_free_disk_mb` は無い（operator の確認、2026-11-11）。書き換えは不要。もし書いてあれば消すか `min_free_disk_pct` に置き換える（残すと新しい binary は読み込みエラーで起動しない）。
- 既定の `min_free_disk_pct = 10` で、300 GB の `/local` は空き 30 GB（使用率 90%）で新しい run を止める。
- `[scratch] forecast_disk_ratio` の既定 1.0 は、btrfs の圧縮（zstd:1）のある `/local` では見込みを多めに数え、Rust の task の並列度が下がる。operator の計測（12:09Z〜12:59Z に見かけ約 188 GB の書き込みで disk 上の使用は約 44 GB 増）からは 0.25 前後だが、同じ時間に消えた file を数えていないので未確定。下げるかどうかは人が決める（例: 1 週間 `celerisctl scratch status` の `admission.held` と `/local` の使用率を見てから 0.5 にする）。
- release-build の target が今 64 GiB を超えていても、次の release の開始時（従来どおり）と終了時（新しい D3）に上限で作り直される。人の手作業は要らない。

## 未解決事項

- 待たせている task の理由は `TaskWait` には出していない（task-core の変更を避けた）。画面の task の待ちの理由には出ず、`celerisctl scratch status` と `GET /api/v1/metrics/scratch` だけに出る。

## 提案

- `TaskWait` に空きの見込みの待ち（例 `DiskForecast { need_bytes }`）を足し、task の画面に待ちの理由を出す（task-core の変更なので別 task）。
