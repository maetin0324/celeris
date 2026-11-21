---
title: credentiald 試験 7 の second_stage_strict と旧挙動の試験を消す（G7 撤去 cred-test）
tasks: [01M7ZYFB4YSJHWR7SF97G501S8]
status: done
updated: 2026-11-21
---
# credentiald 試験 7 の second_stage_strict と旧挙動の試験を消す

完了日: 2026-11-21。基準は `$CELERIS_WU_BASE`。範囲は `crates/celeris-credentiald/tests/injection_peer_uid.rs` と本ファイルだけ。

## 変更（injection_peer_uid.rs）
- `second_stage_strict()`・`SECOND_STAGE_ENV`・`LAUNCHER_ENV`・`GateEnv` を削除。`missing()` は `-> !` で常に panic（env を読まない）。呼び出し側の `return` も削除。G2・G3 の形に戻した。
- `#[ignore]` の理由文から `CELERIS_LAUNCHER_TESTS=require` を除いた（cfg の規則 `userns_tests_cfg.rs` はこの env を読まない）。
- 旧挙動を固定する試験 `test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs` を削除。理由: G7 の移行の契約（env が無ければ PASS）を廃止したため。
- `test9_fails_instead_of_skipping_when_sudo_is_unusable` は env を立てない 1 回の実行にした（`GateEnv` の loop を廃止）。`test9_fails_instead_of_skipping_when_no_probe_line_returns` も env 無し。
- 追加: `test9_fails_under_no_new_privs_without_any_env`。守る不変条件は「env を立てない子でも NoNewPrivs 1・sudo 拒否の文脈で 2 段目の欠けは FAIL」。既存の 2 試験は sudo が exit 1 の形で、NoNewPrivs 1 の子と本物の文言を見ないため、旧試験が見ていた文脈を撤去後の契約で残す。

## 証拠（提出時の HEAD で実行）
- `cargo test -p celeris-credentiald --test injection_peer_uid` → exit 0、7 passed・2 ignored（userns の本体と helper は走らせない）。通った試験: `test9_fails_instead_of_skipping_when_sudo_is_unusable`、`test9_fails_instead_of_skipping_when_no_probe_line_returns`、`test9_fails_under_no_new_privs_without_any_env`、`probe_line_round_trips`、`probe_found_after_libtest_prefix`、`probe_read_from_real_child_run`、`same_uid_control_is_worker_not_allowed`。
- 失敗注入: `missing()` の panic を `eprintln!("SKIP: (not passed) ..."); std::process::exit(0);` に差し替え（strict 相当の無効化）→ 同じ command が exit 101、`test9_fails_instead_of_skipping_when_sudo_is_unusable`・`test9_fails_instead_of_skipping_when_no_probe_line_returns`・`test9_fails_under_no_new_privs_without_any_env` の 3 件が FAILED（4 passed; 3 failed）。注入は元に戻し、再実行で 7 passed。
- `git grep -nE "second_stage_strict|SECOND_STAGE|LAUNCHER_TESTS" -- crates/celeris-credentiald/tests/injection_peer_uid.rs` → 0 件。
