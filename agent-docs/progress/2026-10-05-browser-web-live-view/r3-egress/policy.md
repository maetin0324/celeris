---
tasks: [01M481T9T6AEH4N7MTGXFVKV7V]
status: done
completed: 2026-10-06
---

# R3 policy: ADR 付記と試験専用 loopback egress 許可（既定 off・完全一致のみ）

## やったこと

- ADR `agent-docs/adr/2026-10-05-browser-department-web-live-view.md` 末尾に
  『## 付記: 試験専用 egress 許可と拒否理由の記録（2026-10-06）』を追加。E1（許可の形）・
  E2（本番一致の fail-closed 判定: launcher 側は config path / socket / state_dir の正規化一致、
  daemon 側は ADR-0126 A1 の本番 DB・token 判定で試験許可を申告した launcher を拒否）・
  E3（拒否記録の形と置き場所 `<session_root>/<session_id>/egress-denied.jsonl`）。
- `task_core::browser_isolation::EgressPolicy` に `test_loopback_allow`（serde default、空なら出さない）を追加。
  `test_loopback_target` を公開し、`check_egress` の Connect は host が `127.0.0.1` そのもの・port が
  0/53/853 以外・集合に完全一致のときだけ Ok。他の判定は不変。
- `browser_egress`: 許可された loopback literal は事前検査を通し、DNS を引かず `127.0.0.1:<port>` へ直結。
- `EgressPolicy` のリテラル（`browser.rs`・`browser_launcher/backend.rs`）に空集合を追加（挙動不変。
  launcher config から集合を埋めるのは launcher-cfg 葉）。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p task-core -p task-worker -E 'test(/egress_test_loopback/)'` | 6 passed（task-core 4・task-worker 2）、exit 0 |
| `cargo nextest run -p task-core -p task-worker -E 'test(/browser_isolation\|browser_egress\|egress_test_loopback/)'` | 47 passed、exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | 4114 passed・0 failed、exit 0 |
| `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `check-doc-layout.sh scripts/dev/docs-layout.tsv` | 各 exit 0 |

## 未解決事項

- launcher config からの読み込みと本番一致での起動拒否（E2）は launcher-cfg 葉、拒否記録（E3）は denial-record 葉。
