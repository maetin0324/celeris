---
tasks: [01M481T9T6AEH4N7MTGXFVKV7V]
status: done
completed: 2026-10-06
---

# R3 denial-record: egress 拒否理由を session ごとに記録

## 実装

- proxy は拒否時に `kind`（EgressDenied の snake_case、構文不正は `malformed`）と解析済みの `host`・`port` だけを固定 JSON として stderr に 1 行出す。本文・header・DNS 応答・upstream error は含めない。
- launcher 側 relay は proxy の stderr を最大 512 byte 読み、時刻（UTC RFC3339）と session id を付け、`<state_dir>/sessions/<session_id>/egress-denied.jsonl` に追記する。session_root は launcher が state_dir の直下に固定する。
- 記録ファイルは browser 起動前に排他的に 0600 で作る。親が開いた file descriptor を保持し、既存ファイルがあれば起動を拒否する。最大 128 件・各行 512 byte で、超過時は `truncated` を 1 行だけ記録する。
- ADR 付記 E3 を記録の形式、上限、排他作成に合わせた。台本による `egress-denied.json` への収集は後続の close unit が担当する。

## 試験

- `egress_denial_record_reasons_and_no_request_secrets`: NotAllowed・IpLiteral・PrivateAddress、host・port・UTC 時刻・session id、header と本文の目印および DNS 応答の非記録。
- `egress_denial_record_allowed_connection_has_no_entry`: loopback listener への許可された接続で記録ファイルが空。
- `egress_denial_record_malformed_and_bounded`: 構文不正は kind のみ、件数・行長上限。
- `egress_denial_record_rejects_preexisting_file`: 先に作られたファイルを開かない。
- 既存の実 proxy process 試験は stderr の固定 JSON を検査するよう更新した。これらは userns を使わない。

## 検証結果

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-worker egress_denial_record_ --quiet` | 4 passed、exit 0 |
| `cargo test -p task-worker --test browser_egress_process --test browser_egress_relay --quiet` | 8 passed、exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh` | 4125 passed・0 failed・14 ignored、exit 0 |
| `sh scripts/dev/check-doc-links.sh`、`check-adr-numbers.sh`、`check-doc-layout.sh`、`progress-index.sh --check` | すべて exit 0 |
