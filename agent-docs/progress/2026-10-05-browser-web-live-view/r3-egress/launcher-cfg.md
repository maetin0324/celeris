---
tasks: [01M481T9T6AEH4N7MTGXFVKV7V]
status: done
completed: 2026-10-06
---

# R3 launcher-cfg: launcher config の試験 loopback 許可と本番一致での fail-closed

## やったこと

- `BackendConfig.test_loopback_allow`（`crates/task-worker/src/browser_launcher/backend.rs`、省略時は空 = off）。
  各項目は `127.0.0.1:<port>`（10 進・先頭 0 なし・1〜65535・53/853 以外）だけを受け、他は config 読込みで拒否
  （`validate_test_loopback`）。session の `EgressPolicy.test_loopback_allow` は session の `allowed_domains`
  （`origin_host_port`）と config の集合の交わりだけ（`egress_policy`）。
- launcher（`crates/task-worker/src/bin/celeris-browser-launcher.rs`）: 集合が空でないとき、config path・socket
  （socket activation は `getsockname` の path）・state_dir を本番固定値（`/etc/celeris-browser/launcher.toml`・
  `/run/celeris-browser/launcher.sock`・`/var/lib/celeris-browser`）と正規化して比べ、一致（state_dir は配下・
  祖先も、正規化不能も一致扱い）なら listen 前に exit 1（`refuse_test_loopback_in_production`）。有効時は
  stderr に `test-only loopback egress enabled: …` を出す。
- protocol に `hello` を追加（`Request::Hello {}` / `Response::Hello { protocol_version, test_loopback_allow }`）。
  `SessionBackend::test_loopback_allow`（既定は空）を launcher が申告する。
- daemon 側: `bootstrap::browser_launcher_refuses_test_loopback` / `launcher_test_loopback_refusal`（本番 P が決まらない・
  本番 config を読んだ・`judge_worker_db_guard` が本番 DB/token と判定、のどれかで拒否）。結果を
  `BrowserRuntimeKind::Launcher { refuse_test_loopback }` に入れ、`LauncherRuntime::start_guarded` が
  `start_session` の前に `hello` を送って、申告が空でない・答えない launcher とは session を作らない。
- ADR 付記 E1/E2 を実装に合わせて具体化（欄名・形の検査・交わり・stderr・state_dir の配下判定・hello の形・
  本番判定の 3 条件・launcher binary を同じ release に入れ替えること）。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run --workspace -E 'test(/egress_test_loopback_/)'` | 13 passed（この葉の追加 6: launcher 4・daemon 3 のうち bootstrap 1）、exit 0 |
| `cargo check --workspace --all-targets` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | 4121 passed・13 skipped・0 failed、exit 0 |
| `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `check-doc-layout.sh scripts/dev/docs-layout.tsv` / `progress-index.sh --check` | いずれも exit 0 |

追加した試験（userns なし）:
- (a) `egress_test_loopback_default_off_when_config_omits_it`
- (b) `egress_test_loopback_policy_is_intersection_with_session_domains`・`egress_test_loopback_config_rejects_anything_but_127_0_0_1_port`
- (c) `egress_test_loopback_refuses_production_config_socket_or_state_dir`
- (d) `egress_test_loopback_production_db_or_config_daemon_refuses_test_launcher`（celeris bootstrap）・
  `egress_test_loopback_production_daemon_refuses_test_launcher`・`egress_test_loopback_production_daemon_refuses_launcher_without_hello`（task-worker）

## 未解決事項

- 本番の daemon は `hello` に答える launcher を要る。この版を本番に入れるときは `/usr/local/libexec/celeris/celeris-browser-launcher`
  も同じ release に入れ替える（人の手順。close 葉で docs/ops/browser-launcher-host-setup.md に書く）。
- 実 launcher（userns・root 所有 config）での起動拒否の実機確認は未実施（sandbox では userns 不可）。

## 提案

- なし。
