---
tasks: [01M3YB21F07GQKTRYPRVN184AR]
status: done
completed: 2026-10-03
---
# broker-sandbox: credentiald broker 試験を worker sandbox で通す

対象: `crates/celeris-credentiald/tests/broker.rs` の `daemon_rejects_worker_secret_retrieval_even_with_valid_lease`。

## 原因
- userns・systemd の前提ではない。worker run の環境には `CELERIS_CREDENTIALD_DATA_DIR=/local/celeris/state/credentiald`（本番の vault・audit の置き場、ADR-0136）が入っている。
- 試験は子の `celeris-credentiald serve` に `HOME` と `XDG_RUNTIME_DIR` だけを上書きし、残りの環境を継がせていた。`main.rs` の `data_dir()` はこの変数を優先するため、子 daemon は試験の一時 HOME ではなく本番の data dir を開いた。
  - 本番 vault が既にあり、試験の一時 HOME の鍵と合わない → `initialize_key` が失敗する（broker.rs:367 の `init.success` が false）。
  - init が通る場合でも audit が本番 dir に書かれ、一時 HOME の `.local/celeris/credentiald/audit/journal.jsonl` が無い（:485 で NotFound）。
- host では、この変数が無い shell で走らせていたので通っていた。旧版の試験は sandbox の中から本番の audit journal に grant を書きうる。そのため、これは flaky ではなく本番 data dir の汚染の危険でもある。
- XDG_CONFIG_HOME・XDG_DATA_HOME・XDG_STATE_HOME は daemon が読まない（`grep env::var crates/celeris-credentiald/src` で読むのは HOME・XDG_RUNTIME_DIR・CELERIS_CREDENTIALD_DATA_DIR だけ）。

## 直し方（試験だけ。daemon 本体の src と検査は変えていない）
- `serve` の子: `env_clear()` のうえで、試験の一時 `HOME`・`XDG_RUNTIME_DIR` だけを渡す。
- `bridge` の子: `env_clear()` のうえで一時 `HOME`・`XDG_RUNTIME_DIR` を渡す。
- python3 の worker process: `env_clear()` のうえで `PATH` だけを渡す。
- `initialize_key` が失敗したら reply の `code` を assert の message に出す（`IpcReply` に message 欄は無い）。
- 次の assert は消していない: secret を取り出せないこと、`trusted_injection_required`、journal に SENTINEL が無いこと、grant 2 件・use 0 件。ignore にもしていない。

## 証拠（worker sandbox の中、2026-10-03）
- 修正前の版（`git checkout` で戻した状態）: `cargo test -p celeris-credentiald --test broker daemon_rejects` → panicked at broker.rs:367:5、1 failed
- 修正後: `cargo test -p celeris-credentiald --test broker` → exit 0、12 passed
- 別の値に向けた場合: `CELERIS_CREDENTIALD_DATA_DIR=/nonexistent/x XDG_CONFIG_HOME=/nonexistent/c XDG_DATA_HOME=/nonexistent/d XDG_STATE_HOME=/nonexistent/s cargo test -p celeris-credentiald --test broker daemon_rejects` → 1 passed
- `cargo build --workspace --bins && env -u CELERIS_USERNS_TESTS cargo test --workspace` → exit 0。137 suites、3526 passed、0 failed、13 ignored
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` → exit 0

## 未解決事項・提案
- 旧版の試験を sandbox で走らせた run は、本番の `/local/celeris/state/credentiald/audit` に試験の grant 行を残した可能性がある。人が journal を確かめることを提案する（この run では本番 data に触れていない）。
- daemon を子として起こす他の試験も、`env_clear` で継承環境を断つ形に揃えることを提案する。
