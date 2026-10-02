# 試験用 DB の daemon と userns 試験の opt-in — sandbox 検証

tasks: [01M3YB21F07GQKTRYPRVN184AR]

ADR-0126 の実装（guard-scope / e2e-harness / userns-optin / gate-env / prompt-rule）を worker sandbox（user
namespace を作れない環境）の中で検証した記録。コードは変更していない。

## 前提

- main（`7b77f17a`）を `git merge main --no-edit` で取り込んだ。衝突なし（`docs/PROGRESS.md` に 8 行追加のみ）。
  `release 準備失敗の切り分け（0d438ec1）` 節は merge 後も残っている（削除や衝突は起きなかった）。
- `cargo build -p celeris -p celerisctl` → exit 0（1m02s）。

## 実行コマンドと結果

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
- `cargo test --workspace`（nocapture あり/なし）いずれも `0 failed`。再実行は 2 回とも同じ結果（3278 passed / 0
  failed / 12 ignored）で揺れなし。

既知の高負荷 flaky（`task-dispatch` の `cluster_job_wait` / `build_cache`）は今回の `cargo test --workspace` で
失敗しなかった（単独再実行は不要）。`ignored` の 12 件は手動試験・実クラスタが要る試験（`CELERIS_E2E_SCCACHE` や
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

- 本 run では userns が使える host での `CELERIS_USERNS_TESTS=1` 実行そのものは行っていない（sandbox 制約の
  ため）。上記手順で人が確認する。
- `browser_launcher_ptrace.rs` は未作成（[docs/progress/userns-tests.md](userns-tests.md) に既存の既知事項として
  記載済み、本 task の範囲外）。

## 提案

- release/verify gate の記録に `CELERIS_USERNS_TESTS` の値が残っているかどうかを、次回の selfdeploy 検証時に
  一度確認し、gate-env の実装どおりかを裏取りするとよい。
