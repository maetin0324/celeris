---
title: 試験用 DB の daemon と userns 試験の opt-in — sandbox 検証
tasks: [01M3YB21F07GQKTRYPRVN184AR]
status: done
updated: 2026-10-03
---
# 試験用 DB の daemon と userns 試験の opt-in — sandbox 検証

**最終結果:** verify-3（commit `8f62a105`）で `env -u CELERIS_USERNS_TESTS cargo test --workspace` は exit 0（125 test target、3526 passed / 0 failed / 13 ignored）。以前の exit 101 は修正前の記録であり、解消済み。`worker_db_guard_refuse_with_opt_out` により `worker_read_only` の設定に関わらず、worker run 内で本番 DB path または本番 token に当たる daemon を起動しない fail-closed 動作を固定している。

ADR-0126 の実装を worker sandbox で検証した記録。最終検証は verify-3 commit `8f62a105` で実行した。試験は外部ネットワークへ出ない。

## 最終検証（worker sandbox、2026-10-03）

| コマンド | exit | 結果 |
| --- | ---: | --- |
| `cargo build --workspace --bins` | 0 | 全 workspace binary build 成功 |
| `cargo test --workspace` | 0 | 125 test target、3526 passed / 0 failed / 13 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | warning 0 件 |
| `cargo fmt --all -- --check` | 0 | 差分なし |

`cargo test --workspace` に含まれる `api_scenarios` は 12 passed、`instance_handoff` は 8 passed、`worker_db_read_only` は 1 passed。`instance_handoff` は一時 DB の daemon を実際に起動して完走した。`worker_db_read_only` の 1 件は実 userns が必要なので、既定では理由を出して早期 return する。`cargo test -p e2e --test worker_db_read_only -- --nocapture` で `SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を確認した（exit 0）。この試験の `ok` は実 userns の保護試験が実行された意味ではない。

指定された個別検査も最終 SHA `8f62a105` で成功した。

- `cargo build --workspace --bins && cargo test -p e2e --test api_scenarios && cargo test -p celeris --test instance_handoff` → exit 0（api_scenarios 12 passed、instance_handoff 8 passed）。
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- 統合前の検査（commit `261eb98a` の workspace test exit 101、commit `eb2aa210` の instance_handoff 5 件失敗）は、以下の gate・guard 修正前の履歴であり解消済み。最終 verify-3 の workspace 全試験は exit 0。

### 索引から移した旧 retry 記録

共有索引 `agent-docs/PROGRESS.md` に残っていた retry 記録の詳細は、ここへ移した。commit `261eb98a` の workspace 検査では `worker_db_read_only` が失敗し、sandbox の user namespace 制約で一時 DB daemon が probe に失敗していた。commit `e3d615d3` で指定検査を再実行した時点では workspace 全体未実行で、task-dispatch の時間依存 flake と browser launcher の `/proc` 観測失敗を個別確認していた。その後 userns gate、guard 修正を統合し、verify-3 `8f62a105` で workspace 全試験を再実行して exit 0 を確認した。旧記録にある exit 101 はこの修正前の経緯であり、現在の失敗ではない。

統合時の作業記録では、初回の `cargo test --workspace -- --skip a_wait_parks_the_task_polls_and_resumes_as_a_continuation` は exit 101（worker DB read-only の試験が失敗）、`cargo test -p task-worker --test browser_launcher_ptrace` も exit 101（5 passed、1 failed。Chrome PID が sandbox 内 `/proc` で見つからない）だった。`cargo test -p task-dispatch --lib cluster_job_wait` は単独で 5 passed、`cargo clippy --workspace -- -D warnings` は成功。guard-fix 後は `cargo build --workspace --bins`、workspace test（指定の時間依存 test を除外）、`cargo test -p celeris --lib worker_guard`、`cargo test -p celeris --test worker_db_guard_refuse`、`cargo test -p celeris --test instance_handoff`、clippy を通した。後続修正で browser/userns 試験も既定 opt-in にした最終結果が上記 verify-3 である。

本番 DB/token の拒否は既存の `worker_db_guard_refuse_action_never_probes_inside_worker_run`、`worker_db_guard_refuse_with_opt_out` と `task-worker` の path/token 判定試験が workspace 試験に含まれる。試験用一時 DB の免除経路は `worker_guard_exempt_daemon_starts_on_a_test_db_with_the_guard_on` が確認する。4 件の `task-worker` lib 試験も実 userns が必要だったため、既存の `skip_unless_userns_tests` を適用した。`cargo test -p task-worker --lib db_guard::tests::` は 29 passed / 0 failed。

### この run で解消した失敗（履歴）

- 最初の `cargo test --workspace` は exit 101。`worker_db_read_only` が親 run に worker marker が無いと userns gate を通らず、sandbox の読み取り専用 filesystem 制約で `probe.txt` を作れなかった。親 marker に関係なく `CELERIS_USERNS_TESTS=1` を要求するよう修正した。
- 2 回目も exit 101。`task-worker` lib の実 userns を使う `db_guard_tests` 4 件が `Operation not permitted` で失敗した。4 件に同じ opt-in gate を適用した。
- 上の最終 `cargo test --workspace` は両修正後の再実行で exit 0。以前記録した `instance_handoff` の 5 件失敗は、今回の最終実行では再現しなかった。

## userns 試験を実行する場所

worker sandbox では user namespace を作れないため、この run では `CELERIS_USERNS_TESTS=1` の実行はしていない。ADR-0126 に従い、userns が使える host で `unshare -U -r true` を確認してから `CELERIS_USERNS_TESTS=1 cargo test --workspace` を実行する。`scripts/selfdeploy/release.sh` の gate も `CELERIS_USERNS_TESTS=1` を付ける。実行結果はその host の gate 記録で確認する。

## 未解決

- userns が使える host での opt-in 試験の実行は本 run の sandbox では確認できない。上記の host 側手順が必要。
