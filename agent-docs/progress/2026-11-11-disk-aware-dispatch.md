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
7. 不変条件と試験: `dispatcher/tests/disk_forecast.rs`（待つ・戻れば始まる・続きは増分・0 本なら通す・log 1 回・通知なし・同じ tick にまとめて来た run は lease が出るまでの tick でも見込みに入る分だけ始まる）、`disk_admission.rs` の単体試験、`config/tests.rs`（旧い欄の拒否・範囲）、`scripts/selfdeploy/tests/release_target_settled_after_build.sh`。

## 変更

- D1: `crates/task-dispatch/src/disk_admission.rs`（新）。`空き − Σ 走っている target の増分 − 通したがまだ走査に出ない run の見込み − この run の見込み ≥ 予備` でなければ run を始めず `ready` のまま待たせる（lease・attempts の前）。見込みは target が pool にある同じ `repo_key` の owner の測った大きさの最大（GC の推定と同じ `task_worker::scratch::repo_max_sizes`）。増分を数えるのは run か review の検査が今書いている owner だけ（`target_written_now`）。続きの run（owner の target がある）は増分だけ。target を今書いている run（走査の書いている owner と、通したがまだ走査に出ない run）が 0 本なら通す。待ち始めに 1 回だけ `tracing::info!`（`disk forecast: run held`）。通知・受信箱には書かない。呼び出しは `dispatch_run.rs` の write-set の判定の直後（planner・Remote の run は除く）。
- D2: `[dispatch] min_free_disk_mb` を `[dispatch] min_free_disk_pct`（既定 10、範囲 [0, 100)）に置き換えた。下限 = 容量 × pct。`check_disk_space`（`disk_admission::find_low_disk`）・scratch の GC と実効上限（`run_gc` の `min_free_bytes`）・seed の更新の保留・`celerisctl scratch` が同じ下限を使う。`[scratch] forecast_disk_ratio`（既定 1.0、範囲 (0, 1]）を足した。
- D3: release-build の target が上限を超えて残る理由: `sd_release_prune_enforce_limit` は release.sh の build の前にしか呼ばれず、build で育った target（本番 88.0 GB）はそのまま lease に残っていた。adopt の経路は原因ではない（adopt の後も開始時の確認は通る）。release.sh の EXIT trap に `sd_release_target_settle`（この build の開始時刻より古い workspace の成果物の刈り込みと上限の確認）を足した。
- D4: `ScratchStatus.admission`（`ScratchAdmissionView`: 空き・予備・係数・増分の合計・repo の見込み・owner ごとの見込み・待たせている run）。`GET /api/v1/metrics/scratch` と `celerisctl scratch status`（`--json` と表の両方）に出る。待たせている run（`held`）の正本は daemon の `Gate` のメモリだけで、daemon の snapshot が `Gate::with_held` で埋める。`celerisctl scratch status` の `held` は `null`（表は「only the daemon knows」と API を示す）。API schema（`docs/api/v1/api-v1.schema.json`）と web の生成型を作り直した。
- attempt 3: review の不合格 1 件と申し送り 2 件を直した。(1) `docs/ops/local-disk-growth.md` の `targets_max_gb` の上げ幅が旧い 5 GiB の下限で計算した「約 190」のままだった。下限 30 GiB を 1 倍で引いた 270 GiB（`targets_max_gb` は約 179 まで）に直し、旧い 3 倍の余白は D1 の見込みが守ると書いた。(2) 見込みの材料（`samples_from_owners`）が target の消えた lease の記録も数えていた。`has_target` で絞り、repo の最大を GC の推定と同じ関数（`repo_max_sizes`）にした。(3) class `p0` をすべて増分に数え、lease の生きた release-build・まだ始まらない WU も増分を持っていた。分類の理由で絞り（`target_written_now`: `task running`・`task reviewing`・`work unit running`）、WU の理由を `work unit running` と `work unit queued` に分けた。API の説明が変わったので schema と web の生成型を作り直した。
- attempt 6: review の不合格 1 件と申し送り 2 件を直した。(1) `Gate::begin_pass` が通した量を pass ごとに 0 に戻していたので、直前の tick で通した run の見込みが lease の現れるまで（本番で多くは 10〜30 秒）消え、同じ tick にまとめて来た Rust の task が見込みを超えて始まった。通した run を (task, WU) ごとに `Gate` のメモリに持ち、走査に今書いている owner として出るまで、または dispatcher の `running` から消えるまで数える。(2) 「1 本は通す」の数えを dispatcher の全 run から target を今書いている run に変えた（Rust でない run が走っていても、release-build の予約だけで Rust の run が止まり続けない）。(3) `ScratchAdmissionView` の `owners`・`running_growth_bytes`、`ScratchOwnerForecastView` の `running`・`size_bytes`、試験 `samples_count_present_targets_and_grow_only_where_a_run_writes_now` の説明を phase 5 の実装に合わせ、schema と web の生成型を作り直した。ADR に付記を足した。
- attempt 7: review の不合格 1 件を直した。「1 本は通す」の数え（`Gate::hold` の `writers`）に lease の生きた release-build が入っていたため、task の run が 0 本でも writers が 1 以上になり、空きが予備を割る間 Rust の task がすべて待たされ得た。writers から `release-` の owner を外した（release は見込みの増分には数える）。試験 `a_live_release_build_does_not_stall_the_first_task_run` を足し、ADR の該当文を直した。
- attempt 8（run 01M77617KG）: review の不合格 1 件と申し送りを直した。(1) 「1 本は通す」の書き手を走査の DB の状態（`task reviewing` を含む）から数えていたので、`reviewing` で委譲した子・policy の人の承認・再同期の停止を待つ task が 1 本あるだけで、走っている run が 0 本でも Rust の新しい run が待ち続けた（親が子を待つと deadlock）。書き手を、この daemon が run・review・WU の検査を走らせている task（dispatcher の `running`・`reviewing`・`checking`。`Gate::begin_pass` の `runs`・`others`）の、走査で増分を持つ owner と、通したがまだ走査に出ない run に限った。phase 7 の `release-` の接頭辞での除外は消した（release の owner には task が無い）。`Sample`・`ScratchOwnerForecastView` の `running` を `grows`（増分の予約だけを意味する）に改め、schema と web の生成型を作り直した。(2) 判定の owner を run の鍵（phase のある WU は `Some`）から作っていたので、Task の worktree を共有する WU（並列 1・repair）の続きが見込み全体で判定され、通した分が走査の owner と一致しなかった。worker と同じ `task_worker::scratch::Owner::of_run` に、WU の worktree を決めた後の `cargo_target_work_unit` を渡す（判定を WU の worktree の用意の後へ移した。worker もこの関数を使う）。試験 `a_parent_waiting_in_review_does_not_stall_its_child`（dispatcher）・`a_parent_waiting_in_review_is_not_a_writer_but_a_running_review_is`・`a_shared_worktree_work_unit_is_judged_by_the_task_owner`（Gate）を足した。ADR に付記を足した。health の `source-size`（inline の試験 300 行）と `size.responsibility-growth`（`dispatch_one`・dispatcher.rs の行）に収めるため、`disk_admission` の試験を `disk_admission/tests.rs` に移し、`begin_pass` は走っている run の鍵の集合を受ける形にした。
- 文書: ADR-0075 に付記、`docs/ops/local-disk-growth.md`・`docs/ops/build-tmp-hygiene.md`・`docs/architecture-map.md`・`config/celeris.example.toml`。

commit（件名）:

- `phase 1: hold runs whose cargo target would not fit the disk forecast, make the free-space floor proportional, cap release-build after the build`
- `phase 1: keep the dispatcher module from growing (gate behind &self, disk paths label in disk_watch) and break the scratch_gc cycle`（health の `size.responsibility-growth`〈`dispatch_one` と dispatcher module の行〉と `deps.cycle`〈disk_admission ↔ scratch_gc〉を直した。`dispatch_one` の disk critical の path の組み立てを `disk_watch::paths_label` に移し、`find_low_disk` は statvfs の関数を引数で受ける）
- `phase 1: 進捗に検査の結果を書く`
- `phase 2: keep the held runs only in the daemon's memory and drop the leftover min_free_disk_mb references`（attempt 2。review の指摘 2 件を直した。(1) 初版は待たせている run を pool の `admission.json` に書き、daemon の status もそれを読んでいた。verify の daemon は同じ pool で dispatch の pass を回すので、空の一覧で本番の status を上書きした。file を廃止し、daemon の snapshot がメモリから埋める。記録の刈り込みは候補を照合する pass だけで行う。(2) 消した欄 `min_free_disk_mb` が `tests/e2e/tests/account_pool_scenarios.rs` の試験 config に残り、release gate の `nextest --workspace` で celeris が読み込みエラーになっていた。`min_free_disk_pct = 0` に直し、`DispatchConfig.min_free_disk_pct`・`disk_watch.rs`・celerisctl の `scratch_tests.rs`・`dispatcher/tests/build_cache.rs` の説明も直した）
- `phase 2: 進捗に attempt 2 の検査の結果を書く`
- `phase 3: count only present targets for the repo forecast, grow only targets a run writes now, fix the targets_max_gb bound`（attempt 3）
- `phase 3: 進捗に attempt 3 の検査の結果を書く`
- `phase 5: count the owners that are still copying their seed and the live release-build lease in the forecast`（attempt 5。review の指摘 2 件。(1) seed を写している run は `target/` が無く増分 0 になった。`running` の owner は target が無くても大きさ不明の増える owner として数える。(2) release-build は build ごとに約 88 GB 育つので、lease の生きた release-build の増分を数える。ADR の付記に書いた）
- `phase 6: keep the forecast of admitted runs until their lease shows in the scan, count only target writers for the never-stall rule`（attempt 6）
- `phase 6: 進捗に attempt 6 の検査の結果を書く`
- `phase 7: 1 本は通す数えから release-build の lease を外す`
- `phase 8: count only the tasks this daemon runs as writers for the never-stall rule, judge with the worker's scratch owner`（attempt 8）
- `phase 8: move the disk_admission tests out of line and pass the running run keys to the gate (health size limits)`
- `phase 9: point the celeris script-compare test config at its own releases_dir (host releases leaked a release_unpromoted inbox item)`（attempt 9。review の不合格は celeris の `script_compare_cos_digest` 6 件だけ。原因は試験の共通 config〈`crates/celeris/tests/support/script_compare.rs` の `config_text`〉が `[selfdeploy] releases_dir` を書かず、既定の `~/.local/celeris/releases`〈host〉を dispatcher の `release_unpromoted` の判定が読んでいたこと。host に verify 済みで昇格されていない release〈82b753db3d63、verify.json 05:35Z、promoted.json なし〉があると受信箱に `release_unpromoted-selfdeploy` が増え、byte 一致で比べる world が変わる。他の celeris の試験〈credential_wait_notify・campaign_*_e2e・unpromoted_release〉と同じく `releases_dir = "releases"`〈一時 dir〉に向けた。守る不変条件: 試験は host の release の状態に依らない。既存の試験で検出できなかった理由: 失敗が host の状態に依るため〉

## 証拠

（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- attempt 9（code の変更の後、進捗の記録の前）: `bash scripts/dev/test-affected.sh` exit 100（1268 秒、開始時 load average 約 24、途中 60〜80）。nextest 6771 件のうち 6769 passed。前回落ちた `script_compare_cos_digest` 6 件は passed。落ちたのは範囲外の負荷依存 2 件: task-worker `ssh::tests::remote_command_times_out_and_kills_the_ssh_process_group`（孫の `sleep` が 10 秒以内に消えない）と celeris-campaign `k_cases_grade::k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one`（600 秒の上限で timeout。host の原票 tgz を展開して再計算する試験）。この 2 件は同じ -p の組で単独に走らせると 2 passed（k3_k4 は 92 秒、load 約 47）。web 113 passed。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` exit 0。
- attempt 8（提出時の HEAD）: `bash scripts/dev/test-affected.sh` exit 100（1300 秒）。nextest 6764 件のうち 6758 passed、落ちたのは celeris の `script_compare_cos_digest` の 6 件だけ（web 711 passed、repo-health verdict=pass）。この 6 件は同じ binary を単独で走らせても落ちる（9 件中 6 failed）。差は、受信箱に余分な `release_unpromoted-selfdeploy` の項目があること（host の release の状態から来ると見ている。この変更は selfdeploy・受信箱の経路を変えていない。main で同じ失敗が出るかは時間切れで未確認）。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` は exit 0（code の最後の変更の前に実行）。`bash scripts/dev/check-health.sh` verdict=pass。
- attempt 8: `cargo nextest run -p task-dispatch -p task-worker -p task-api -p task-ops -p celerisctl -p celeris -E 'test(disk_admission) | test(disk_forecast) | test(scratch) | test(committed_schema_matches_generated)'` 90 passed（新しい試験 3 件を含む）。`UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` 1 passed、`node web/scripts/gen-types.mjs` で生成型を作り直した。

- attempt 7（提出時の HEAD）: `bash scripts/dev/test-affected.sh` exit 0（nextest 6761 passed）。初回は負荷依存の task-worker `daemon_post_login_consent_not_pressed_twice_and_diagnostics_hold_no_page_data` 1 件が落ち（範囲外）、再実行で exit 0。clippy・fmt・progress-index exit 0。`cargo test -p task-dispatch --lib disk_admission` 8 passed（新しい試験を含む）。
- attempt 6（提出時の HEAD、main は動いておらず rebase なし）: `bash scripts/dev/test-affected.sh` exit 0（901 秒、開始時 load average 約 31）。nextest 6751 passed・23 skipped、web 103 file・710 passed、repo_health_gate all checks passed。`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`sh scripts/dev/progress-index.sh --check` ok。`cargo nextest run -p task-dispatch --lib -E 'test(/disk_forecast/) | test(/disk_admission/)'` 11 passed（新しい試験 `admitted_runs_count_across_passes_until_the_scan_shows_them`・`runs_admitted_in_one_tick_keep_their_forecast_until_their_lease_appears` を含む）。`begin_pass` で通した run を毎回消す旧い挙動に一時的に戻すと、この 2 件だけが落ちる（9 passed・2 failed）ことを確かめてから戻した。`UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` 1 passed、`node web/scripts/gen-types.mjs` で生成型を作り直した。
- attempt 5（提出時の HEAD、main は動いていない）: `bash scripts/dev/test-affected.sh` exit 0（nextest 6749 passed。1 回目は負荷で `instance_handoff` の 1 件が落ち、単独実行 8 passed・再実行で全件 passed）。clippy・fmt・progress-index exit 0。task-dispatch・task-worker の scratch・disk 系 71 passed。
- attempt 4（コード変更なし、main は動いていない）: 前回の review で落ちた `script_compare_cos_digest`（celeris）は単独実行で 9 passed。提出時の HEAD の `bash scripts/dev/test-affected.sh` は exit 0（nextest 6749 passed・開始時 load average 約 21）。負荷が高い時に時間依存で落ちる既存の試験で、この変更の範囲外。
- attempt 3（main は動いておらず rebase なし）: `bash scripts/dev/test-affected.sh` exit 0（1189 秒、開始時 load average 約 36）。nextest 6749 passed・23 skipped、web 103 file・710 passed、repo_health_gate all checks passed。`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`sh scripts/dev/progress-index.sh --check` ok。`cargo nextest run -p task-worker -p task-dispatch --lib -E 'test(/scratch/) | test(/disk_forecast/) | test(/disk_admission/) | …'` 116 passed（新しい試験 `samples_count_present_targets_and_grow_only_where_a_run_writes_now`・`only_running_and_reviewing_owners_count_as_written_now` を含む）。`UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` 1 passed、`node web/scripts/gen-types.mjs` で生成型を作り直した。
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
- `[scratch] forecast_disk_ratio` の既定 1.0 は、btrfs の圧縮（zstd:1）のある `/local` では見込みを多めに数え、Rust の task の並列度が下がる。operator の計測（12:09Z〜12:59Z に見かけ約 188 GB の書き込みで disk 上の使用は約 44 GB 増）からは 0.25 前後だが、同じ時間に消えた file を数えていないので未確定。下げるかどうかは人が決める（例: 1 週間 `GET /api/v1/metrics/scratch` の `admission.held`〈待たせている run。`celerisctl scratch status` は pool の走査だけで組むので `held` は `null`〉と `/local` の使用率を見てから 0.5 にする）。
- release-build の target が今 64 GiB を超えていても、次の release の開始時（従来どおり）と終了時（新しい D3）に上限で作り直される。人の手作業は要らない。

## 未解決事項

- 負荷が高いとき（load average 60 以上）に落ちる範囲外の試験: task-worker `ssh::tests::remote_command_times_out_and_kills_the_ssh_process_group`、celeris-campaign `k_cases_grade::k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one`（600 秒超過）。単独では通る。この変更は ssh・campaign の判定を変えていない。

- 待たせている task の理由は `TaskWait` には出していない（task-core の変更を避けた）。画面の task の待ちの理由には出ず、daemon の `GET /api/v1/metrics/scratch` の `admission.held` だけに出る（`celerisctl scratch status` は見込みだけを出し、`held` は `null`）。
- 増分の予約（`grows`）は DB の状態から決まるので、`reviewing` で子を待つ親・WU を待つ `running` の親の target も増分を予約する（書き手ではないので詰まりはしないが、予約が多めになり Rust の run が待つ時間が延びうる）。

## 提案

- `TaskWait` に空きの見込みの待ち（例 `DiskForecast { need_bytes }`）を足し、task の画面に待ちの理由を出す（task-core の変更なので別 task）。
