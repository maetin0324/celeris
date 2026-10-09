---
title: browser 適合台帳の credential 証拠（isolation_suite・egress_negative_suite）の生成と台帳への載せ方（close-out）
tasks: [01M4F73EKB2FFGAKRY2041RZ49]
status: done
updated: 2026-10-09
completed: 2026-10-09
---

# browser 適合台帳の credential 証拠（close-out）

ADR `agent-docs/adr/2026-10-09-browser-ledger-credential-evidence.md`（状態: 実装済み。突き合わせは末尾の付記）。
葉ごとの進捗は `2026-10-09-browser-ledger-credential-evidence/<wu-key>.md`
（adr・merge-ledger-fix・gen-credential・selfdeploy-wiring・certify-credential）。

前提の branch `ops/ledger-fix`（bf757bff）は `merge-ledger-fix` で main 側の統合 branch に入り、その後
`integrate-base` で ADR と合わせた。credential 段は `gen-credential`・`selfdeploy-wiring`・`certify-credential` の順に入った。

## WU ごとの証拠

| WU | 状態 | 主な証拠 |
|---|---|---|
| adr | done | ADR 追加。`crates/`・`scripts/` の差分なし |
| merge-ledger-fix | done | `ops/ledger-fix` を no-ff merge。`browser_loopback_prod_build` 1 passed、browser filter 163 passed / 0 failed / 3 ignored、task-worker clippy exit 0 |
| gen-credential | done | 生成器 `--credential-evidence`。`scripts/tests/test_browser_conformance_credential.py` 9 件 OK（偽 cargo。実 cargo・userns は未使用） |
| selfdeploy-wiring | done | `sd_browser_ledger` に credential 段。`browser_ledger_credential_evidence.sh` 合格（`browser_ledger_release_stages.sh` を含む） |
| certify-credential | done | `browser_ledger_credential_` 1 passed、`browser_backend::tests::` 8 passed、celerisctl `browser_ledger_release_` 8 passed |
| integrate-impl（統合 HEAD `fde3f018`） | done | integration-checks の test-parallel: nextest 4882 passed / 0 failed、doctest 0 失敗 |

## close-out での検証（HEAD `fde3f018`、コードは変更していない）

- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。
- `TMPDIR=/tmp/cet bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY`: nextest passed 4882、failed 0、ignored 14、doctest exit 0、`summary_parsed: true`、`tmp_leftovers: 0`。
  - 既定の TMPDIR（93 文字の run dir）で最初に流したときは 63 件が `path must be shorter than SUN_LEN` で落ちた（exit 100）。
    これは既存の Unix socket 試験の path 上限で、この変更とは無関係。短い TMPDIR で流し直して全件合格。
- 文書検査（`agent-docs/README.md` の land 系 check）:
  - `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。
  - `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (170 files)`、exit 0。
  - `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0。
- 参考: `python3 -I scripts/dev/check-architecture-map.py` は exit 1（`docs/architecture-map.md` の 3 件のパス表記）。
  このブランチはこの file を変えていない（最後の変更は main 側の `b3e7c93b`）。この WU の 3 本の検査には含まれず、本件の範囲外として残す。
- 生成器と Rust の試験名の一致: `scripts/browser-conformance.py` の `P4A_ISOLATION_TESTS` + `P4A_EGRESS_NEGATIVE_TESTS`（40 件）と
  `crates/task-core/src/browser_backend.rs` の 2 つの定数（19 + 21 件）を正規化して比べ、集合が一致（gen-credential の「残作業」を解消）。

## 人が実行する手順（配送後・本番 host）

この手順は人が実行する。エージェントは本番の台帳・release を書き換えない（ADR-0095 付記 D-d）。
台帳の生成には userns が使える host が要る（`CELERIS_USERNS_TESTS=1` の試験を流すため）。

1. 対象 release の sha12 を確かめる（例: 現在の release。`ls ~/.local/celeris/releases/`）。
2. 台帳を作り直す（release 差し替えはしない。既存の台帳は、新しい台帳が置けなかったときは残る）:
   ```
   scripts/selfdeploy/browser-ledger.sh <sha12> --force
   ```
   全体の上限は既定 3600 秒（`SD_BROWSER_LEDGER_TIMEOUT`）。試験 40 件を 1 件ずつ起動するので数十分かかり得る。
3. 結果を確かめる。`~/.local/celeris/releases/<sha12>/browser/ledger-status.json` を見る:
   - `credential_evidence.ok` が `true`、`code` が `ok`。
   - `credential_backends` に `claude-code` と `browser-specialist` が入る（空なら credential_use は `ledger_lacks_credential` で止まる）。
   - 失敗のとき: `code` と `reason` を読む。`tests_not_run` は userns・launcher 不足（環境の問題）、`tests_failed` は
     `browser/credential/credential-logs/` の該当 log を読む。`p4b_incomplete` は先に P4-B の証拠を揃える。
4. 台帳そのものの確認: `~/.local/celeris/releases/<sha12>/bin/celerisctl browser ledger check --file ~/.local/celeris/releases/<sha12>/browser/conformance.json --release <sha12> --agent-browser agent-browser --json`
   と `celerisctl browser doctor`。
5. 結果を task `01M4F73EKB2FFGAKRY2041RZ49` のコメントか運用記録に残す（`ledger-status.json` の `credential_backends` と `credential_evidence` の値）。

## 未解決事項

- 本 run では本番 host の台帳を作っていない。実機で `credential_evidence.ok: true` になるかは未確認（userns・launcher・別 UID の daemon が要る）。
- `launcher_chrome_denies_daemon_uid_ptrace` は daemon UID 1001 相当・launcher socket・subuid を要る（ADR-0109 解放条件 1）。揃わない host では credential は certify されない（fail closed の意図どおり）。
- 既定の TMPDIR で `test-parallel.sh` を流すと SUN_LEN で 63 件落ちる（既存の問題。前項のとおり短い TMPDIR で回避）。TMPDIR を長くしうる運用では直す価値がある。
- `check-architecture-map.py` の 3 件（前項）。

## 提案

- `test-parallel.sh` 自身が長い TMPDIR を検知して短い dir を使うようにする（既存の生成器と同じ手当て）。これで手動の回避が要らなくなる。
- 台帳の作り直しを `release.sh` の後に自動で走らせるかは、人の判断（本番操作に当たるため、今は手順のまま）。
- 生成器の試験名一覧が 2 か所（Python と Rust）にあるため、両者の一致を確かめる試験（`test_browser_conformance_credential.py` の Rust 読み取り試験）を CI 相当の検査に入れておくと安全。
