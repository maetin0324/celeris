---
task: 01M4ZRKYMX6FWH0VCWY3GXPX94
wu: jev-key
status: done
completed: 2026-10-15
---
# jev-key: secrets dir の jev_api_key を読む ApiKeyProvider

- `crates/celeris/src/daemon/jev_key.rs` `SecretsDirKeyProvider`（ADR jev-wire D2）。呼び出しごとに読み直し、未登録・空・dir 無しは `None`（→ `KeyMissing` → credential 待ち）。Debug は secret id のみ。
- 試験 5 件: 登録/未登録/再登録、Debug に値が出ない、未登録は credential、401/403 は credential（bearer で送信・board に値なし）、429/402/500 は credential にならない。
- 証拠: `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace --all-targets -- -D warnings` 警告なし / `test-parallel.sh` passed 5358 failed 0。
- 未解決: daemon への構築・配線は jev-tick。`daemon.rs` の `#[allow(dead_code)]` はそのとき外す。
