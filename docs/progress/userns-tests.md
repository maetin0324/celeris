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

この一覧以外にも ADR-0126 B1 に記載した `crates/task-api/tests/` の browser 統合試験、および実 namespace を作る unit 試験（worker DB guard、scratch、container、browser、task-dispatch の該当試験）がある。`browser_launcher_ptrace.rs` は未作成。

## 既定で走る daemon 試験

`crates/celeris/tests/instance_handoff.rs` と `crates/e2e/tests/api_scenarios.rs` は一時 DB・一時 config を使い、userns を要しないため opt-in 対象から除外する。
