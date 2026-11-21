---
title: task-worker 試験 7 の second_stage_strict・旧挙動の試験を消す（G7 撤去 worker-test）
tasks: [01M7ZYFB4YSJHWR7SF97G501S8]
status: done
updated: 2026-11-21
---
# task-worker の G7 の分岐の撤去（WorkUnit worker-test）

完了日: 2026-11-21。担当は `crates/task-worker/src/db_guard_tests.rs` だけ。

## 変更

- `SECOND_STAGE_ENV`・`second_stage_strict()` を消した。`ssh_config_includes_still_parse_inside_the_namespace`（試験 7）は、
  2 段目の前提（ssh・`/etc/ssh/ssh_config.d`）が欠けたら常に panic する。env は読まない。
- 旧挙動を固定する試験 `ssh_test_keeps_old_behaviour_without_the_gate_env` を消した（契約の G7 を廃止したため）。
- `run_test7_without_ssh` から `strict` 引数と env の操作を消した。`ssh_test_fails_instead_of_skipping_when_ssh_is_missing` は env 無しの子で FAIL を確かめる。
- 試験 9（`test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs`・`LAUNCHER_ENV` の読み取り）は
  `crates/celeris-credentiald/tests/injection_peer_uid.rs` にあり、cred-test の担当。このファイルには無い。
  task-worker に残る `CELERIS_LAUNCHER_TESTS` は launcher の試験（`browser_launcher/userns.rs`、`tests/browser_launcher_ptrace.rs`）で、G7 とは別の用途なので触っていない。

## 検証

- `cargo test -p task-worker --lib ssh_`: exit 0、9 passed・1 ignored（ignored は cfg 無しの試験 7 本体）。
- 失敗注入: 試験 7 の `panic!` を `eprintln!("skip"); return;` に戻して `cargo test -p task-worker --lib ssh_test_fails`: exit 101、
  `ssh_test_fails_instead_of_skipping_when_ssh_is_missing ... FAILED`（"child must FAIL, not pass"）。確認後に戻した。
- `git grep -n "second_stage_strict\|keeps_old_behaviour\|SECOND_STAGE" -- crates/task-worker`: 0 件。

## 本番への移行

config・DB・unit の変更は無い。
