---
title: target-sweep sweep-cli（celerisctl target sweep と release.sh 呼び出し）
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# sweep-cli

- `celerisctl target sweep [--root <path>]... [--dry-run | --apply] [--json]`（`crates/celerisctl/src/commands/target_sweep.rs`）。
  既定は dry-run、`--dry-run` と `--apply` は排他。`task_dispatch::target_sweep::run_sweep` を呼ぶだけで DB は開かない。
- `--root` 省略時は `default_roots(<build_cache_dir>)`（`<dir>/cargo` と `/var/tmp/agent-platform-build`）。config は読まない
  （`$HOME/.local/celeris/build-cache` を仮の `build_cache_dir` に使う）。`[maintenance.target_sweep].roots` との接続は sweep-cron。
- `scripts/selfdeploy/release.sh`: 梱包（bin コピー）直後、`.lock-release` 保持中に
  `"$STAGE/bin/celerisctl" target sweep --apply --root "$SD_CARGO_TARGET"` を 1 回。失敗は `sd_log "warning: …"` のみ。

## 証拠

- `cargo test -p celerisctl target_sweep_cli` → 3 passed（dry_run_json_deletes_nothing / apply_with_root / default_roots）
- `cargo run -p celerisctl -- target sweep --help` → --root/--dry-run/--apply/--json が出る
- `bash -n scripts/selfdeploy/release.sh` → exit 0
- `cargo clippy -p celerisctl --all-targets -- -D warnings` → 警告なし

## 提案

- sweep-cron 葉で config の roots を `default_roots` と差し替えて繋ぐ。
