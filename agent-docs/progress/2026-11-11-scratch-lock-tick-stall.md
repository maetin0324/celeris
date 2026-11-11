---
title: scratch の pool lock による tick 停止と消えた handoff 要求
tasks: [01M757GNJA20FHCF54DED64RBT]
status: running
updated: 2026-11-11
---
# scratch の pool lock による tick 停止と消えた handoff 要求

## 変更

調査と [設計 brief](../adr/2026-11-11-scratch-lock-tick-stall.md) を記録。実装中。task 全体は未完了。

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

## 呼び出し元の調査

- `scratch.rs::allocate_with_seed`: 同期。通常 run は `worker_task` の spawn_blocking 内。`housekeeping::check_cargo_target_env` と `eval_trials::eval_allocate_target` は tick の同期経路で、変更が必要。
- `scratch.rs::touch/set_ttl/release`: CLI は待機可。評価試行の touch/release/set_ttl は tick から呼ぶため変更が必要。
- `maintenance::run_target_sweep_task`: tick の同期経路。Busy なら Ready のまま次回へ。
- `scratch_gc::execute`: tick の通常・緊急 GC と CLI。Busy なら削除を次回へ。
- `scratch/seed.rs::seed_gc`: tick。Busy なら次回へ。
- `scratch/seed.rs::refresh_seed`: 更新スレッド。pool lock は公開と旧世代の退避に使用。コピー中の参照保護と調整が必要。
- `DaemonSnapshot.last_tick_at` は既存。health/log の露出は未確認。

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
- `bash scripts/dev/test-affected.sh` は attempt 2 では未実行（壁時計の残りが足りない）。

## 未解決

- 評価試行（`eval_trials::eval_allocate_target`）は tick の中で `allocate_with_seed`（seed の写しと pool の `.lock` の待ち）と `set_ttl` を呼ぶ。`score_eval_trial` の `touch`、片付けの `release` も pool の `.lock` を待つ。未修正（受け入れ条件 4 は未達）。
- 実 daemon 系の試験 fixture が実 host の disk watch を継承する問題を解消し、指定検査を完走する。
- task 全体は未完了。完了日は未設定。

## 提案

無し。
