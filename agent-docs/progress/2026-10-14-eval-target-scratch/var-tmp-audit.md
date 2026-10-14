---
title: 葉 var-tmp-audit — /var/tmp/agent-platform-build を既定に持つ経路の洗い出しと撤去
tasks: [01M4WHYHVZTYNBRGM5C3PWHMZE]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# 葉 var-tmp-audit

## 洗った結果（`git grep agent-platform-build`・`/var/tmp`・`CARGO_TARGET_DIR=`）
- 残っていた既定は `target_sweep`（ADR 2026-10-07-build-tmp-hygiene D1.1）の既定 roots の 2 つ目だけ。3 か所:
  - `crates/celeris/src/config/maintenance.rs` の `DEFAULT_DEV_BUILD_CACHE`（`[maintenance.target_sweep]` の `roots` 省略時）
  - `crates/task-dispatch/src/dispatcher/maintenance.rs` の `DEFAULT_DEV_BUILD_CACHE`（`set_target_sweep` が無いときの fallback）
  - `crates/celerisctl/src/commands/target_sweep.rs` の `DEFAULT_TMP_ROOT`（`--root` 省略時）。HOME が無いときの
    `/var/tmp/celeris-build-cache` もルート fs なので同時に撤去。
- `crates/task-core/src/model.rs:2016` は event の round-trip 試験の値だけ（経路ではない）。scratch の path に変えた。
- ふだんの task run: ADR-0075 の scratch target を `CARGO_TARGET_DIR` で受ける（この置き場を使わない）。
- release・selfdeploy: `release.sh`・`browser-ledger.sh` は `SD_CARGO_TARGET`（既定 `$SD_RELEASES/.cargo-target`、`/local`）を
  `CARGO_TARGET_DIR` で渡す。/var/tmp を使う経路なし。
- 評価試行（`eval_trials.rs`）と `scripts/dev` は別葉（eval-target・adr-script）なので触っていない。

## やったこと
- ADR `2026-10-14-eval-trial-scratch-target` の D3 に洗った結果と決定を書いた（実装の前）。
- `task_worker::target_sweep::{DEV_TARGETS_SUBDIR, default_roots}` を足し、既定 roots を
  `<build_cache_dir>/cargo` と `<scratch>/dev-targets`（D2 の script の置き場）にした。3 か所はこれを使う。
  - daemon: `<[scratch] dir>/dev-targets`（`Config::scratch_dir()`）。
  - dispatcher の fallback: scratch が有効なときだけ `dev-targets` を足す（`fallback_sweep_roots`）。
  - celerisctl: `<CELERIS_SCRATCH_DIR（既定 /local/celeris/data/scratch）>/dev-targets`（script と同じ決め方）。
- `config/celeris.example.toml`・`docs/ops/build-tmp-hygiene.md` を追随。
- 試験: `target_sweep_default_roots_are_build_cache_and_scratch_dev_targets`（task-worker）、
  `target_sweep_fallback_roots_use_scratch_dev_targets_not_var_tmp`（task-dispatch）、
  `target_sweep_config_defaults_match_sweep_params_default`（celeris。`[scratch] dir` 指定も見る）、
  `target_sweep_cli_default_roots`（celerisctl）。どれも roots に `/var/tmp` が無いことを assert する。

## 残した物と理由
- config 欄 `[maintenance.target_sweep] roots` は残す（既定値が変わるだけで、欄の意味は同じ）。読み込みエラーにする欄なし。
- `agent-docs/adr`・`agent-docs/progress`・`agent-docs/ops/dev-builds-local-target-dir.md` の記述は過去の記録・掃除手順なので残す。
- `docs/ops/browser-web-live-check.md` などの `/var/tmp/cb-<UID>` は browser 実機確認の証跡 dir（cargo target ではなく小さい。
  `/tmp` を launcher が覆うため意図して /var/tmp）。範囲外。

## 証拠
- `git grep -n agent-platform-build -- crates config docs` → 0 件（exit 1）
- `cargo test -p task-worker -p celerisctl -p task-core target_sweep` / `cargo test -p celeris -p task-dispatch -- target_sweep` → 全 ok
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 5089 / failed 0 / ignored 13
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all --check` → exit 0

## 本番への移行
- 本番 config の `[maintenance.target_sweep] roots` に `/var/tmp/agent-platform-build` を明示していれば外す
  （書いていなければ何もしない。既定は `<[scratch] dir>/dev-targets` に変わる）。確認: `grep -n roots ~/.config/celeris/config.toml`。
- `/var/tmp/agent-platform-build/` の残骸は `du -sh` で確かめて人が消す。

## 未解決事項
- なし。

## 提案
- なし。
