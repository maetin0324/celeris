# 試験用 DB の daemon と userns 試験の opt-in — sandbox 検証

tasks: [01M3YB21F07GQKTRYPRVN184AR]

ADR-0126 の実装と final review 修正を worker sandbox（user namespace を作れない環境）で検証した記録。

## 初回検証時の記録（履歴。今回の retry 結果は後段）

- main（`7b77f17a`）を `git merge main --no-edit` で取り込んだ。衝突なし（`docs/PROGRESS.md` に 8 行追加のみ）。
  `release 準備失敗の切り分け（0d438ec1）` 節は merge 後も残っている（削除や衝突は起きなかった）。
- `cargo build -p celeris -p celerisctl` → exit 0（1m02s）。

当初の検証では以下が通っていた。retry で全 workspace を再実行したところ、`instance_handoff` の通常 daemon 起動が
一時 DB でも worker run marker を持たないため probe に進み、sandbox の userns 制約で失敗した。したがって初回記録を
今回の成功根拠として扱わず、下記の retry 結果を採用する。

## Final review 修正の試験

- 本番 DB/token 拒否: `cargo test -p celeris --lib worker_db_guard_refuse_action_never_probes_inside_worker_run` → exit 0
  （1 passed）。`RefuseProduction` の worker run 内判定で probe closure が呼ばれないことを固定する。
- e2e の免除経路: `cargo test -p e2e --test api_scenarios worker_guard_exempt_daemon_starts_on_a_test_db_with_the_guard_on`
  → exit 0（1 passed）。worker marker を通し、`worker_read_only` を false にせず、一時 DB の Exempt 起動ログを確認する。
- lib 内 userns gate: `cargo test -p task-worker --lib production_action_path_reaches_fixture_through_real_browser_and_egress -- --nocapture`
  → exit 0（1 passed）。`CELERIS_USERNS_TESTS` 未設定で指定の `SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を表示する。

## Retry の検査結果（worker sandbox）

| コマンド | exit | 結果 |
| --- | ---: | --- |
| `cargo build --workspace --bins` | 0 | 全 workspace binary build 成功 |
| `cargo test --workspace` | 101 | `instance_handoff` 8 件中 3 passed / 5 failed。他 test target の全体集計には到達せず |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | warning 0 件 |
| `cargo fmt --all -- --check` | 0 | 差分なし |

`instance_handoff` の失敗は `normal_mode_does_not_inject_the_smoke_builtins`、`starting_the_same_release_twice_exits_three`、
`verify_mode_never_dispatches_and_never_touches_daemon_instances` が一時 DB に対する probe の `Operation not permitted` で失敗。
`a_newer_release_takes_over_while_the_old_one_finishes_its_run` と `a_stale_heartbeat_promotes_the_standby` も daemon が
userns probe を通過できず handoff 条件を満たせなかった。これは ADR-0126 A3 の「worker run 内の guard 済み sandbox の
一時 DB」の免除対象ではなく、通常 daemon は従来どおり userns probe をするためである。既存 `instance_handoff` は WU test
process に worker marker を渡していない。

`worker_db_guard_refuse_action_never_probes_inside_worker_run`、
`worker_guard_exempt_daemon_starts_on_a_test_db_with_the_guard_on`、および lib 内 gate の個別試験は pass したが、これらは
workspace 全試験の成功を代替しない。

## 初回実行コマンドと結果（履歴）

| コマンド | exit | 結果 |
| --- | ---: | --- |
| `cargo test -p e2e --test api_scenarios` | 0 | 11 passed / 0 failed / 0 ignored |
| `cargo test -p celeris --test instance_handoff` | 0 | 8 passed / 0 failed / 0 ignored（`a_stale_heartbeat_promotes_the_standby` は 60.55s） |
| `cargo test --workspace`（`CELERIS_USERNS_TESTS` 未設定） | 0 | 3278 passed / 0 failed / 12 ignored |
| `cargo clippy --workspace -- -D warnings` | 0 | warning 0 件 |
| `cargo fmt --all -- --check` | 0 | 差分なし |

`cargo test -p e2e --test api_scenarios` と `cargo test -p celeris --test instance_handoff` はこの worker sandbox
（user namespace を作れない）の中で実行し、両方とも daemon の起動から試験の完了まで exit 0 で完走した。worker db guard
は一時 DB・一時 config・試験 token の daemon に対して userns を要求しなかった（ADR-0126 A の判定どおり）。

## userns 試験の skip 確認

`cargo test --workspace -- --nocapture` で再実行し、`CELERIS_USERNS_TESTS` 未設定の下で何件が opt-in 分岐の
`SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を出したかを数えた。

- `grep -c "SKIPPED (userns test, not passed)"` → **18 件**。すべて `test result: ... ok`（`#[ignore]` ではなく
  関数内の早期 return のため「ok」と表示される。カウントは `ignored` 欄ではなく上記 grep で確認した）。
- 対象試験ファイル（`userns_gate` を import するもの）: `crates/task-api/tests/browser_restore_deliver.rs`、
  `browser_h3_injection.rs`、`browser_restore_live_session.rs`、`crates/task-worker/tests/browser_runtime_isolated.rs`、
  `browser_restore_deliver.rs`、`browser_injection_wire.rs`、`browser_runtime_supervisor.rs`、
  `browser_injection_attacks.rs`、`browser_shared_cdp.rs`、`browser_cdp_sink.rs`、`browser_h3_wire.rs`、
  `browser_egress_relay.rs`（一覧は [docs/progress/userns-tests.md](userns-tests.md) と一致）。
- 当時 `cargo test --workspace`（nocapture あり/なし）は 0 failed と記録した（3278 passed / 0 failed / 12 ignored）。
  この履歴は後段の retry 失敗により今回の acceptance を満たす証拠ではない。

既知の高負荷 flaky（`task-dispatch` の `cluster_job_wait` / `build_cache`）は今回のログでは失敗に含まれなかった。
`ignored` の 12 件は手動試験・実クラスタが要る試験（`CELERIS_E2E_SCCACHE` や
`CELERIS_CLUSTER_HOST` 系）で、userns opt-in とは無関係。

## CELERIS_USERNS_TESTS=1 での実行（userns の使える host で人が行う手順）

worker sandbox は user namespace を作れないため、本 run では `CELERIS_USERNS_TESTS=1` を付けた実行は行っていない
（ADR-0126 B の仕様どおり、未設定なら skip のみで fail しないことをこの sandbox で確認した）。userns の使える host
（selfdeploy の release/verify gate や、手元の開発機など unshare -U が通る環境）で検証する場合は、次の手順を人が
実行する。

1. `unshare -U -r true` が exit 0 であることを確認する（userns が使える環境かどうかの事前確認）。
2. その環境で `CARGO_TARGET_DIR` 等いつもの環境変数を設定したうえで
   `CELERIS_USERNS_TESTS=1 cargo test --workspace` を実行する。
3. 上記 18 件の `browser_*` 系試験が `ok`（スキップでなく実行）になり、`0 failed` であることを確認する。
4. `scripts/selfdeploy/release.sh` は ADR-0126 どおり既定で `CELERIS_USERNS_TESTS=1` を付けて実行するため、
   release gate の記録（`prepare.log` / gate 結果）を確認すれば userns 試験の実行結果も残る。

## 未解決

- 本 run では userns が使える host での `CELERIS_USERNS_TESTS=1` 実行そのものは行っていない（sandbox 制約のため）。
- retry の acceptance `cargo test --workspace` は未達。`instance_handoff` の5試験が通常 daemon の userns probe 制約で
  失敗したため、instance 系を worker sandbox で通す条件の整理が必要。
- `browser_launcher_ptrace.rs` は未作成（[docs/progress/userns-tests.md](userns-tests.md) に既存の既知事項として
  記載済み、本 task の範囲外）。

## 提案

- release/verify gate の記録に `CELERIS_USERNS_TESTS` の値が残っているかどうかを、次回の selfdeploy 検証時に
  一度確認し、gate-env の実装どおりかを裏取りするとよい。
