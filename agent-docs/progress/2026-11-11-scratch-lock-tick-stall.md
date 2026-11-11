---
title: scratch の pool lock による tick 停止と消えた handoff 要求
tasks: [01M757GNJA20FHCF54DED64RBT]
status: done
completed: 2026-11-11
updated: 2026-11-11
---
# scratch の pool lock による tick 停止と消えた handoff 要求

## 変更

調査と [設計 brief](../adr/2026-11-11-scratch-lock-tick-stall.md) を記録し、実装した（attempt 3 で完了）。

commit: `phase 1: defer scratch cleanup without blocking tick`。

- `Pool::try_lock` を追加し、maintenance・通常 GC・seed GC の待つロックを置き換えた。Busy の maintenance は Dispatch せず Ready を保つ。
- 待機可能な `Pool::lock` は1秒以上待ったとき、呼び出し位置と待機秒数を warn に出す。try-lock の Busy は debug に出す。
- 別 fd の pool lock を保持したまま tick/GC が戻る試験を追加。保険の30秒で解放された場合は失敗し、通常は channel の通知で直ちに解放する。解放後の掃除も確認する。
- 通常 run は既に `worker_task` の spawn_blocking 内。停止経路は checks 用 env の準備にもあり、ここは未修正。

commit: `phase 2: copy the seed outside the pool lock and drop handoff requests from gone requesters`（attempt 2）。

- `allocate_with_seed` は pool lock の下で世代の pin（`pin_current_seed`、世代の `.copy-pin` に共有 flock）と lease の書き込みだけを行い、写しは lock を外して行う。同じ owner の割り当ては `.alloc.lock` で pool lock の外で直列にする。seed の GC は pin のある世代とその repo を退避せず、owner の GC は `.alloc.lock` が持たれた owner を消さない。
- handoff: active は要求元（自分より新しく drain していない、pid が生きている standby・active の行）が居るときだけ draining になる。居なければ要求を消して active に残る（判定はどちらも書き込みの transaction の中）。standby は要求の前に自分の行を書く。draining と要求の破棄の log に `request_age_secs` を出す。
- 最後の tick の時刻は既存の `GET /daemon` の `snapshot.last_tick_at` で読める（流用。新しい欄は足していない）。
- 検査の env（`housekeeping::check_cargo_target_env`。呼び出し元は housekeeping・work_units・review_spawn・phase_integration）は `scratch::try_allocate_for_check` を使う。owner の `.alloc.lock` と pool の `.lock` を try で取り、取れなければ lease に触らず target の path だけを返す。seed は写さない（target が無ければ空で作る）。試験 `check_allocation_does_not_wait_for_the_pool_lock_nor_copy_the_seed`。

commit: `phase 3: allocate eval trial targets off the tick and stop waiting for the pool lock`（attempt 3）。

- 評価試行の target（`eval_allocate_target`: `allocate_with_seed` と `set_ttl`）を、通常の run と同じく run の future の `spawn_blocking` で adapter の起動の前に行う。tick は target の path（`pool.target_dir(owner)`）だけを request に入れる。割り当ての失敗は `AdapterError::Other("cargo target: …")` として既存の `on_eval_worker_finished` が試行を `error` にし、作業ツリーと target を消す。
- 採点の `touch` と片付けの `release` は新しい `scratch::try_touch`・`scratch::try_release`（pool の `.lock` が使用中なら待たずに `Ok(false)`。release は lease を TTL に任せたことを info に出す）。待つ版の `touch`・`release` は CLI（`celerisctl scratch lease`）だけが使う。
- owner の `.alloc.lock`（`lock_owner_alloc`）も 1 秒以上待ったら owner の dir と待機秒数を warn に出す。
- review の軽微な指摘 2 件を直した: `reflink.rs` の `remove_partial_copies` の doc comment の位置、`instance.rs` の `DropStaleHandoff` の log 文字列の空白。
- 試験 `eval_trial_tick_does_not_wait_for_a_held_pool_lock`（別の fd で pool の `.lock` を握ったまま `tick_eval_trials` と `tick` が返り、run は lock が空くまで始まらず、空いたら target と TTL 付きの lease ができて run が始まる。採点と片付けも lock を握ったまま終わり、target は消え、lease は TTL に残る）。既存の試験 2 つ（`eval_trial_target_removed_on_recovery`・`start_hung_eval_trial`）は、target を run の開始の前に用意するようになったので、target を見る前に run の開始（adapter の `started`）を待つ。

## 呼び出し元の調査

- `scratch.rs::allocate_with_seed`: 待つ。通常 run は `worker_task` の spawn_blocking 内、評価試行は run の future の spawn_blocking 内（attempt 3）。`housekeeping::check_cargo_target_env` は `try_allocate_for_check`（attempt 2）。
- `scratch.rs::touch/set_ttl/release`: 待つ版は CLI と評価試行の spawn_blocking（set_ttl）だけ。tick の評価試行は `try_touch`・`try_release`（attempt 3）。
- `maintenance::run_target_sweep_task`: tick の同期経路。Busy なら Ready のまま次回へ。
- `scratch_gc::execute`: tick の通常・緊急 GC と CLI。Busy なら削除を次回へ。
- `scratch/seed.rs::seed_gc`: tick。Busy なら次回へ。
- `scratch/seed.rs::refresh_seed`: 更新スレッド。pool lock は公開と旧世代の退避に使用。コピー中の参照保護と調整が必要。
- `DaemonSnapshot.last_tick_at` は既存で、`GET /daemon` の `snapshot.last_tick_at` で読める。
- 結論: tick（main thread と tick の中の同期処理）から pool の `.lock` や owner の `.alloc.lock` を待つ経路は残っていない。

## 証拠（提出時の HEAD のコードで実行。基準は git merge-base HEAD main）

- `cargo nextest run -p task-dispatch -p task-worker -E 'test(target_sweep_tick_defers) | test(gc_dry_run_lists) | test(seed_gc_keeps)'` → exit 0、3 passed。後続の影響範囲試験でもこの3件の失敗は無し。
- `cargo nextest run -p task-worker -E 'test(launch_notice_tests)'` → exit 0、4 passed。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → fixture 修正後の再実行も exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（記録更新後に再実行）。
- `bash scripts/dev/test-affected.sh` → 初回は health の規模増加で exit 1。整理後の health は pass、worsened 0。
- 影響範囲試験の1回目は exit 100、5,877件中5,874 passed・3 failed（起動通知 fixture。隔離後の4件は上記のとおり合格）。
- fixture 修正後の影響範囲試験は exit 100、5,877件中5,850 passed・27 failed・22 skipped（約340秒）。実 daemon 系で失敗が出ており、受け入れ条件0は未達。ログに `/local` 使用率95.16%、`disk watch: critical; new coding runs are held` がある。既定の95%閾値で試験の coding run が保留されている。本番の設定・容量は変更していない。

## 検査で見つかった試験環境の修正

`launch_notice_tests` の Codex fixture が継承した `CODEX_HOME` を使い、起動前の rules 作成を host の account に向けていた。fixture の owner で試験専用の home を設定し、既存の3つの通知契約を保った。新しい fallback・本番の変更は無し。既存の試験で期待値は変えず、`run-order` の adapter の戻り値も失敗メッセージへ出す。

attempt 2 の検査（提出時の HEAD で実行）:

- `cargo nextest run -p celeris --lib -E 'test(/instance::tests/)'` → 29 passed（`a_handoff_request_from_a_gone_requester_is_dropped_and_the_active_stays` を含む）。
- `cargo nextest run -p celeris --test instance_handoff --test unpromoted_release` → 18 passed（実 daemon の handoff）。
- `cargo nextest run -p task-worker --lib -E 'test(/scratch::tests/)'` → 42 passed（`seed_copy_runs_without_the_pool_lock_and_gc_keeps_the_pinned_generation` を含む）。
- `cargo nextest run -p task-dispatch --lib -E 'test(/scratch_gc/) | test(target_sweep_tick_defers)'` → 9 passed。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 5,880 passed・22 skipped（747 秒）、repo-health は verdict=pass・new 0・worsened 0。実行時の /local 使用率は 94%、load average は 117。

attempt 3 の検査（提出時の HEAD で実行。基準は git merge-base HEAD main）:

- `cargo nextest run -p task-dispatch --lib -E 'test(/eval_trial|eval_recovery|eval_run|live_handoff_keeps_running_eval/)'` → exit 0、21 passed（新しい `eval_trial_tick_does_not_wait_for_a_held_pool_lock` を含む）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0（fmt の commit の後）。
- `bash scripts/dev/test-affected.sh` → exit 100。nextest 5,881 件中 5,880 passed・1 timed out・22 skipped（777 秒）。時間切れは `celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`（600 秒）の 1 件で、この branch は celeris-campaign を変えていない。実行時の load average は 85、/local の使用率は 86%。repo-health は verdict=pass・new 0・worsened 0、repo_health_gate は all checks passed。
- 時間切れの試験の単独の再実行 `cargo nextest run -p celeris-campaign --test k4_cells_validity -E 'test(real_k4_archive_grades_thirty_of_thirty_two)'` → exit 0、1 passed（392 秒）。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

## 未解決

- test-affected の全体は、範囲外の試験 1 件の負荷による時間切れで exit 100 だった（上記）。負荷の低い時に再実行すれば通る見込み（未確認）。
- 評価試行の hidden check（`run_eval_check`）と target の `remove_dir_all` は tick の中の同期処理として残る（pool の `.lock` は待たない。この task の範囲外）。

## 提案

- 評価試行の hidden check と target の削除も tick の外（spawn_blocking）へ移す task を起こす。数分の cargo の検査が tick の中で走ると、pool の `.lock` と同じく tick が止まる。
