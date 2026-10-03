# userns 試験の opt-in 対象一覧

tasks: [01M3YB21F07GQKTRYPRVN184AR]

ADR-0126 B に基づき、実 user namespace または隔離 browser/runtime を使う統合試験は、既定で skip する。実行する場合は `CELERIS_USERNS_TESTS=1` を指定する。`CELERIS_ISOLATION_TESTS=skip` は互換のため引き続き優先し、`CELERIS_ISOLATION_TESTS=require` と `CELERIS_DB_GUARD_TESTS=require` も opt-in として扱う。

## 統合試験

`crates/task-worker/tests/` の次の試験は共通 helper `userns_gate` で外側の実 userns/browser 実行試験を gate する。

- `browser_injection_wire.rs`
- `browser_runtime_isolated.rs`
- `browser_shared_cdp.rs`
- `browser_egress_relay.rs`
- `browser_h3_wire.rs`
- `browser_cdp_sink.rs`
- `browser_runtime_supervisor.rs`
- `browser_injection_attacks.rs`
- `browser_restore_deliver.rs`
- `browser_launcher_ptrace.rs`（main から取り込み。実 launcher session を起こす `launcher_chrome_denies_daemon_uid_ptrace` だけを gate する。`CELERIS_LAUNCHER_TESTS=require` も従来どおり opt-in として扱い、gate を通った後に launcher socket・subuid が無ければ従来どおり理由を出して skip、require なら fail。`/proc` の合成 table と stand-in process で組む `chrome_pick_*`・`admission_table_on_synthetic_launcher_observation` は userns を使わないので既定で走る）

この一覧以外にも ADR-0126 B1 に記載した `crates/task-api/tests/` の browser 統合試験、および実 namespace を作る unit 試験（worker DB guard、scratch、container、browser、task-dispatch の該当試験）がある。lib 内では main から来た `crates/task-worker/src/browser_launcher/userns.rs` の `real_namespace_when_host_is_ready`（実 `unshare(CLONE_NEWUSER)`）を `test_support::skip_unless_userns_tests` で gate した（`CELERIS_LAUNCHER_TESTS=require` も opt-in）。

`tests/e2e/tests/worker_db_read_only.rs` は実 DB guard の読み取り専用 mount を作るため、親 run の worker marker の有無に関係なく既定 skip とする。`crates/task-worker/src/db_guard_tests.rs` の実 namespace 試験にも同じ gate を適用する。どちらも `CELERIS_USERNS_TESTS=1` で実行する。

## main から来て gate しない試験（userns 不要と確認）

- `crates/task-worker/tests/browser_prod_admission.rs`: 合成の `RuntimeFacts`・証明で admission を判定するだけ。
- `crates/task-worker/tests/reap_finished_children.rs`: 子 process の zombie 回収だけ。
- `crates/celeris/tests/browser_startup_reap.rs`: `sleep` の stand-in process と記録で startup reap を確かめるだけ。
- `crates/celeris/tests/unpromoted_release.rs`: 一時 DB・一時 config の daemon 起動で、unshare・bwrap を使わない。
- `crates/task-worker/src/browser_launcher/tests.rs`・`browser_launcher_run_tests.rs`: socket・stand-in process の試験で、userns を作らない。

## release gate での扱い

`scripts/selfdeploy/release.sh` は `CELERIS_USERNS_TESTS` を既定 1 で export するので、上の追加分も release gate の `cargo test --workspace` の範囲に入る（launcher の host 準備が無ければ従来どおり理由を出して skip、`CELERIS_LAUNCHER_TESTS=require` で fail）。

## 既定で走る daemon 試験

`crates/celeris/tests/instance_handoff.rs` と `tests/e2e/tests/api_scenarios.rs` は一時 DB・一時 config を使い、userns を要しないため opt-in 対象から除外する。
