# Phase 4: atomic coding task の直行経路 — 統合検証

tasks: [01M3Y2KXY3R6YXD6DEDDNFG97T]

完了日: 2026-10-02

## 検証結果

統合後 HEAD `dd4b5a6131b4` で実行。実装コードの変更はなく、architecture map の実装位置表記のみ更新した。

- `cargo build --workspace --bins` → exit 0。
- `cargo test --workspace` → exit 101。`instance_handoff` は 8 件中 3 passed / 5 failed。失敗は `normal_mode_does_not_inject_the_smoke_builtins`、`starting_the_same_release_twice_exits_three`、`verify_mode_never_dispatches_and_never_touches_daemon_instances`、`a_newer_release_takes_over_while_the_old_one_finishes_its_run`、`a_stale_heartbeat_promotes_the_standby`。
- `cargo test -p celeris --test instance_handoff`（単独再実行）→ exit 101。同じ 5 件が再現。前3件のログは ADR-0095 worker db guard の user namespace probe が `Operation not permitted`。後2件は dispatcher/standby 待機 assertion が失敗。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。

## 未解決事項

この sandbox では `unshare -U -r true` 相当の user namespace 作成が拒否され、daemon handoff 統合試験の一部が実行できない。残る2件の handoff assertion も単独再実行で再現したため、環境起因と断定はせず失敗として報告する。コード変更はせず、環境制約と再現結果を記録した。

## 提案

user namespace が利用可能な環境で `cargo test --workspace` を再実行し、`instance_handoff` の5件を再確認する。
