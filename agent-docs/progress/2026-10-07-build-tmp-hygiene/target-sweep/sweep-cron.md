---
title: 葉 sweep-cron — cron extra.action=target_sweep の決定的 executor と [maintenance.target_sweep]
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 sweep-cron

ADR 2026-10-07-build-tmp-hygiene D1.4 の cron 実行を入れた。LLM 呼び出しは無い。

- **設定** `crates/celeris/src/config/maintenance.rs`: `[maintenance.target_sweep]`（`roots`・`max_age_days`・
  `max_bytes_per_root`・`target_ratio`・`stale_target_days`）。省略時は `SweepParams::default()` と既定 roots
  （`<[workspace].build_cache_dir>/cargo`・`/var/tmp/agent-platform-build`）。`Config::target_sweep_params()` が
  executor と CLI 向けの `SweepParams` を返す。相対 roots は設定ファイル基準、`~` 展開。値の検証あり。
  `celeris::Config` に公開欄 `maintenance` が増えたので `celerisctl/src/commands/worker_tests.rs` の直組みに 1 行足した。
- **action の検証** `task_ops::cron_jobs::template_action`: `extra.action` は `target_sweep` / `tmp_sweep` の予約語だけ。
  `tmp_sweep` は「予約語だが未実装」で拒否。`validate_job`（API・seed 投入）と `[[cron.seed]]` の設定検証の両方が通る。
- **発火** `template_to_spec`: action をラベル `maintenance-action-target-sweep` に写す（ラベルは `[a-z0-9-]`）。
  雛形に acceptance が無ければ「executor が 1 回走らせ記録した」の criterion を入れる（task 作成の必須条件のため）。
- **executor** `crates/task-dispatch/src/dispatcher/maintenance.rs`: tick の cron 発火の直後に、作られた task を
  worker に渡さず `run_sweep` を mode（`extra.mode`、既定 dry_run）で呼び、`Event::TargetSweepRan` を追記する。
  遷移は既存の trigger だけ（dispatch → worker_done(+event) → review_pass = done、errors があれば
  worker_error{retryable:false}(+event) = failed）。手動実行・queued 由来の task も `dispatch_ready` の候補で横取りする。
  時計は dispatcher の時計（試験は注入時計）。roots・上限は daemon が起動時と reload で `set_target_sweep` で渡す。
- **例** `config/celeris.example.toml`: `[[cron.seed]] name = "target-sweep"`（enabled=false、`15 4 * * *`、Asia/Tokyo、
  overlap skip、catch_up latest、action target_sweep、mode apply）と `[maintenance.target_sweep]` の既定例。
  例の既存試験 2 本（daily-curation の `[seed]` 分解）を名前で引く形に直した。
- 範囲内の既存 clippy 違反 `crates/celeris/tests/model_role_assignments_consistency.rs:388`（cloned_ref_to_slice_refs）を
  `std::slice::from_ref(&source)` に直した。

## 試験

- `target_sweep_cron_fires_deterministic_executor_and_records_event`（task-dispatch）: 一時 dir の偽 target 木、注入時計、
  worker 枠 1。発火 tick で task が done、`WorkerStarted`・`RoutingDecided` が無い、`TargetSweepRan`（apply・root 1・
  age で削除）が 1 件、古い rlib は消え新しい rlib と実 flock 保持中の release の項目は残る（`build_in_progress`）。
- `target_sweep_cron_rejects_unknown_action`（task-dispatch）: `rm_rf`・`tmp_sweep` を job 作成で拒否。
- `target_sweep_cron_seed_example_toml_passes_validation`（celeris）: 例の種が設定検証と `seed_cron_if_empty`（validate_job）を通る。
- `target_sweep_config_*` 4 本（celeris）: 既定値＝`SweepParams::default()`＋既定 roots、上書きと相対 roots、不正値の拒否、例の値＝既定。
- `cron_seed_rejects_invalid_seeds_at_load` に action の 2 例を追加。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-dispatch target_sweep_cron` | exit 0、2 passed |
| `cargo test -p celeris --lib target_sweep` | exit 0、5 passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0、警告なし |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo test --workspace --no-run` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0、4684 tests run: 4684 passed, 13 skipped |

## 未解決事項

- `celerisctl target sweep` の `--root` 省略時に config の roots を使う配線は、サブコマンドが並行葉 sweep-cli の
  ブランチにしか無いので、ここでは `celeris::Config::target_sweep_params()`（roots 込み）を用意するところまで。
  統合（integrate-run）後に sweep-cli 側の `--root` 省略分岐から `Config::load(..)?.target_sweep_params()` を呼ぶ 1 か所の配線が要る。
- executor は tick の中で同期に掃除する（`remove_dir_all` を含む）。大きな target を消すと tick が長くなる。

## 提案

- 削除だけを `prune_one_workspace` と同じく別スレッドに逃がす（rename までは lock 保持のため同期のまま）。
- executor の失敗理由（`SweepReport.errors`）を event に載せる欄が `TargetSweepRan` に無い。人が failed の理由を
  GUI で読めるよう、`errors_head` を足すか別 event を検討する。
