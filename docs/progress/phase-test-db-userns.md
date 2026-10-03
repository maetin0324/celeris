# 試験用 DB の daemon と userns 試験の opt-in — sandbox 検証

tasks: [01M3YB21F07GQKTRYPRVN184AR]

ADR-0126 の実装を worker sandbox で検証した記録。以下の「最終検証」は、この文書と `docs/PROGRESS.md` の記録更新以外の変更をすべて含むコードで実行した。`CELERIS_USERNS_TESTS` は未設定。試験は外部ネットワークへ出ない。

## 最終検証（worker sandbox、2026-10-03）

| コマンド | exit | 結果 |
| --- | ---: | --- |
| `cargo build --workspace --bins` | 0 | 全 workspace binary build 成功 |
| `cargo test --workspace` | 0 | 125 test target、3526 passed / 0 failed / 13 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | warning 0 件 |
| `cargo fmt --all -- --check` | 0 | 差分なし |

`cargo test --workspace` に含まれる `api_scenarios` は 12 passed、`instance_handoff` は 8 passed、`worker_db_read_only` は 1 passed。`instance_handoff` は一時 DB の daemon を実際に起動して完走した。`worker_db_read_only` の 1 件は実 userns が必要なので、既定では理由を出して早期 return する。`cargo test -p e2e --test worker_db_read_only -- --nocapture` で `SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を確認した（exit 0）。この試験の `ok` は実 userns の保護試験が実行された意味ではない。

本番 DB/token の拒否は既存の `worker_db_guard_refuse_action_never_probes_inside_worker_run`、`worker_db_guard_refuse_with_opt_out` と `task-worker` の path/token 判定試験が workspace 試験に含まれる。試験用一時 DB の免除経路は `worker_guard_exempt_daemon_starts_on_a_test_db_with_the_guard_on` が確認する。4 件の `task-worker` lib 試験も実 userns が必要だったため、既存の `skip_unless_userns_tests` を適用した。`cargo test -p task-worker --lib db_guard::tests::` は 29 passed / 0 failed。

### この run で解消した失敗（履歴）

- 最初の `cargo test --workspace` は exit 101。`worker_db_read_only` が親 run に worker marker が無いと userns gate を通らず、sandbox の読み取り専用 filesystem 制約で `probe.txt` を作れなかった。親 marker に関係なく `CELERIS_USERNS_TESTS=1` を要求するよう修正した。
- 2 回目も exit 101。`task-worker` lib の実 userns を使う `db_guard_tests` 4 件が `Operation not permitted` で失敗した。4 件に同じ opt-in gate を適用した。
- 上の最終 `cargo test --workspace` は両修正後の再実行で exit 0。以前記録した `instance_handoff` の 5 件失敗は、今回の最終実行では再現しなかった。

## userns 試験を実行する場所

worker sandbox では user namespace を作れないため、この run では `CELERIS_USERNS_TESTS=1` の実行はしていない。ADR-0126 に従い、userns が使える host で `unshare -U -r true` を確認してから `CELERIS_USERNS_TESTS=1 cargo test --workspace` を実行する。`scripts/selfdeploy/release.sh` の gate も `CELERIS_USERNS_TESTS=1` を付ける。実行結果はその host の gate 記録で確認する。

## 未解決

- userns が使える host での opt-in 試験の実行は本 run の sandbox では確認できない。上記の host 側手順が必要。
