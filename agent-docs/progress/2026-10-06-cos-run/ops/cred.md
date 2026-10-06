# CoS run credential と checkpoint store

---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
---

- `0052_cos_run_credentials.sql` に run ごとの credential を保存する。全 refs の migration 番号を走査した結果、最大は 0051 だったため 0052 を使用し、0051 はこのブランチでは予約版数とした。
- 発行時は `/dev/urandom` の 32 byte を token にし、DB には SHA-256 digest のみを保存する。同じ run への再発行は conflict。検証は未知・期限切れ・失効を分け、live run の thread/run ID のみを返す。
- 明示失効関数と `chat_run_finish` の同一 transaction 内の失効を追加した。dispatcher から発行関数を呼ぶ配線は後段の chat-run が担当する。
- checkpoint は live run の入力メッセージ seq を配送済み上限とする。割り込みで追い越した未配送 user メッセージが範囲にあれば拒否する。32 KiB・期待 cursor・後退を検査し、要約だけを保存する。
- 試験用に時計を注入できる `_at` 関数を置いた。`cos_chat_ops_cred_` / `cos_chat_ops_checkpoint_` の試験で発行、検証、終端失効、配送範囲、サイズ、競合を確認する。

検証: `cargo test -p task-core --lib --quiet` は 766 passed、`cargo clippy --workspace -- -D warnings` は exit 0。`cargo test -p celerisctl --test no_migrate --quiet` は 4 passed。`CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh` は 4036 passed、0 failed、13 ignored、exit 0。

追試: `Cargo.lock` が WU の範囲 check から外れるため、乱数取得を既存の browser runtime と同じ `/dev/urandom` に統一し、`getrandom` の直接依存を外した。`cargo test -p task-core cos_chat_ops_ --lib --quiet` は 6 passed、`cargo clippy -p task-core -- -D warnings` と `cargo fmt --all --check`、範囲 check は exit 0。

統合時の注意: 0051 の実 migration が入ったら `RESERVED_VERSIONS` から 51 を外し、`migration_sql` に登録する。操作 transaction 側も credential 検証後に run の live 状態を再確認すると、終了との競合で旧 token を使う窓を閉じられる。
