---
title: launcher 経路の action socket path（D4）と shim config/policy（D6）
tasks: [01M47QXZR0QMCYZM9KAZC81BCD]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# launcher 経路の action socket path（D4）と shim config/policy（D6）

WorkUnit `run-path`。設計は [2026-10-06-browser-action-socket-path](../../adr/2026-10-06-browser-action-socket-path.md)。

## 変えたこと

- D4: `browser_action::action_socket_path(session_dir)` を足し、daemon 経路（`browser.rs`）と launcher 経路（`browser_launcher_run.rs`）の両方で使う。置き場所は `/tmp/celeris-browser-<euid>/<sha256(session dir) 先頭 16 桁>.sock`（固定長、dir は 0700・所有者を確認）。107 byte を超えたら bind の前に path と長さを含む `AdapterError` を返す。
- D3（shim 側）: 両経路の `config.json` に `action_socket` を書き、`browser_cli.py` はそれに接続する（無ければ旧来の path）。
- D6: launcher 経路の shim 用 file を `write_shim_files` にまとめ、`config.json` に `policy_sha256` を書く。`policy.json` は daemon 経路と同じ action policy（`launch`・`close` を含む）。
- 追加で見つけた欠陥: launcher の `action_allowed`（`browser_launcher/server.rs`）は URL の host を正規 origin（`https://example.com`）と文字列比較していて、daemon が渡す policy ではどの navigate も通らなかった。`browser_policy::url_origin_allowed` で scheme・host・port を照合する（素の host の旧形式は従来どおり）。

## 試験（偽 launcher・一時 dir、userns・実 browser・実 launcher なし）

- `browser::launcher_run::tests::action_socket_path_is_short_and_fixed_length_for_a_deep_workspace`
- `browser::launcher_run::tests::action_socket_path_over_the_limit_fails_before_bind_with_path_and_length`
- `browser::launcher_run::tests::action_socket_dir_must_be_private_to_this_user`
- `browser::launcher_run::tests::launcher_shim_files_pass_load_policy_and_gate_navigation_by_origin`（実の shim を python3 で走らせ、`load_policy` 受け入れ、`https://example.com` と `http://127.0.0.1:17730` の open が FakeLauncher に `Open` で届き、不許可 origin は shim と action server の両方で拒否）
- `browser_launcher::server::origin_tests::*`（2 件）

## 証拠

- `cargo test -p task-worker --lib -- action_socket launcher_shim origin_tests` → 6 passed
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 4098, failed 0, ignored 14）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → 差分なし

## 未解決事項

- host の `/usr/local/libexec/celeris/celeris-browser-launcher` を入れ替えないと launcher 側の origin 照合の修正は実機に効かない（root の作業）。実機の再確認は Fable。
- egress allow（`backend.rs` の `format!("{d}:443")`）は WorkUnit egress の担当で、ここでは触っていない。

## 提案

- なし。
