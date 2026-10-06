---
tasks: [01M48DGY90GJAW2T8C95267HV3]
---
# F4: browser session HOME と session dir cleanup

## 対応

`bwrap_args` の `HOME` を `/session` から `/session/home` に変更した。Chrome/fontconfig が `.cache` や `.config` を session root 直下へ作らず、browser subuid が書き込める `home` 以下へ置く。

launcher の userns holder cleanup は従来の `output`, `home`, `run`, `actions`, `profile`, `tmp` の内容削除に加え、session root 直下の `.cache`、`.config` などの dot entry も削除する。`.[!.]*` と `..?*` の glob を使い、`.` と `..` 自体は対象にしない。固定 cleanup script を定数化し、userns を使わない単体試験で動作を確認する。

## 検証

- `cargo test -p task-worker --lib browser_launcher::userns -- --nocapture` — pass（4 passed、実 userns 試験は opt-in skip）
- `cargo test -p task-worker --lib browser_runtime::userns_args_tests::home_is_session_root_subdir_not_session_root_itself -- --nocapture` — pass（1 passed）
- `cargo clippy -p task-worker --all-targets -- -D warnings` — pass
- `git diff --check` — pass
