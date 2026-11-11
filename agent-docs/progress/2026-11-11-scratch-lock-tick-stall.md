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

## 未解決

- コピーの参照保護・pool lock の短縮。コピー中の owner の同時割り当て・GC も保護する必要がある。
- `housekeeping::check_cargo_target_env` と評価試行の割り当てを tick 外へ移す。checks 用 env の呼び出し元は housekeeping・work_units・review_spawn・phase_integration。
- 評価試行の touch/release/set_ttl の待機経路を除く。
- handoff 要求元を永続化し、要求元削除時の取消と drain 直前の transaction 内の存在・pid 生存確認を実装する。
- `last_tick_at` を health または log で確認できるようにする。
- 実 daemon 系の試験 fixture が実 host の disk watch を継承する問題を解消し、指定検査を完走する。
- task 全体は未完了。完了日は未設定。

## 提案

無し。
