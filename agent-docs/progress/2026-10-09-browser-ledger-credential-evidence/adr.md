---
task: browser-ledger-credential-evidence
wu: adr
status: done
completed: 2026-10-09
tasks: [01M4F73EKB2FFGAKRY2041RZ49]
---

# adr: ADR 2026-10-09-browser-ledger-credential-evidence を書く

## したこと
- `agent-docs/adr/2026-10-09-browser-ledger-credential-evidence.md` を追加。
  - D1: `isolation_suite`（19 試験）・`egress_negative_suite`（21 試験）を試験名で固定（`<package>:<target>::<libtest 名>`）。
    task-core `browser_isolation::tests`、task-worker の `browser_runtime_isolated`・`browser_runtime_supervisor`・
    `browser_launcher_ptrace`・`browser_egress_relay`・`browser_egress_process`・`browser_egress::tests` から grep で選んだ。
  - D2: 生成器の `--credential-evidence <LEDGER> --credential-backend <id>... --output-dir <DIR>`、1 試験 1 起動、
    `CELERIS_USERNS_TESTS=1`・`CELERIS_LAUNCHER_TESTS=require`・`CELERIS_ISOLATION_TESTS` 除去、skip 印
    （`SKIPPED|^SKIP:|\(not passed\)`）で `not_run`、全件 passed のときだけ 2 件を passed（fail closed）、差し替え env `CELERIS_CONFORMANCE_CARGO`。
  - D3: `sd_browser_ledger` の P4-B の後・check の前。exit 0 のときだけ台帳を採用、他は P4-B 後の台帳を残す（credential_backends 空）。
    `ledger-status.json` に `credential_evidence {ok, code, reason}`。`browser-ledger.sh` は同じ関数。既定 timeout 3600 秒。
  - D4: `required_evidence` を 2 件にも広げ、名前だけでは通らない（`case_passed` の evidence 完全性を要求）。admission は変えない。
- コードは書いていない（crates/・scripts/ の差分なし）。

## 証拠
- `git diff --name-only $CELERIS_WU_BASE -- crates scripts` → 出力なし。

## 未解決事項
- `launcher_chrome_denies_daemon_uid_ptrace` は daemon UID 1001・launcher socket・subuid を要る（ADR-0109 解放条件 1 の別 UID 実証）。
  台帳を作る host でこれが揃わなければ credential は certify されない（意図どおりの fail closed）。
- `ops/ledger-fix` の「本番 build に loopback 例外が無い」試験は名前が統合後に決まるため一覧に入れていない。

## 提案
- `docs/architecture-map.md` の browser 行に本 ADR の索引を足す（close-out で）。
