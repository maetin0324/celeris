# selfdeploy-ledger: release・promote への適合台帳の組み込み

---
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
status: done
updated: 2026-10-08
---

ADR [2026-10-08-browser-prod-enablement](../../../adr/2026-10-08-browser-prod-enablement.md) D1.2・D1.3・D1.5 を `scripts/selfdeploy` に実装した。

- `lib.sh` の `sd_browser_ledger <build> <out> <sha12>`: agent-browser の版確認（`agent_browser_missing` / `agent_browser_version`）→ `browser-conformance.py --protocol-scripted --fallback-scenario --celeris-release` → `--p4b-evidence`（claude-code・browser-specialist。失敗しても公開能力の台帳は残し `p4b:false`）→ `celerisctl browser ledger check --no-host-probe --json` → 通れば `browser.partial` を `browser` に rename。`<out>/browser/ledger-status.json` は失敗時も `{ok:false, code, …}`。全体を `SD_BROWSER_LEDGER_TIMEOUT`（既定 1800 秒）の締切で `timeout --kill-after=30s` に掛ける（code `timeout`）。差し替え env: `SD_BROWSER_LEDGER_RUNNER`・`SD_AGENT_BROWSER`・`SD_BROWSER_LEDGER_CHECK_BIN`・`SD_BROWSER_AGENT_VERSION`。
- `release.sh`: `$STAGE` 組み立て中（web bundle の後、manifest の前）に段を回し `|| true` で非 blocking。`manifest.json`・`gate.json` に `browser_ledger {ok, code}`、ログは `gate-logs/browser-ledger.log`。
- `promote.sh`: 台帳が無くても止めない。`promoted.json` とログに `browser_ledger {ok, code}`（無ければ `missing`）。
- `browser-ledger.sh <sha12> [--force]`: 人が実行する作り直し。有効（`celerisctl browser ledger check` が ok）なら何もしない。build worktree を sha に合わせて `sd_browser_ledger`、成功したら `browser/` を入れ替え。失敗時は既存を残して exit 1（既存が無ければ status だけ置く）。

検証（2026-10-08）:

- `bash scripts/selfdeploy/tests/browser_ledger_release_stages.sh`（`sh` でも通る）: exit 0。未配置（agent-browser 無し・版違い・生成器失敗でも release は exit 0）、配置済み（P4-B 失敗時も公開台帳が残る）、`browser-ledger.sh` の作り直し・有効なら不作為・`--force`・古い台帳・失敗時に既存を残す、を確認。fake の生成器・agent-browser・celerisctl・一時 dir のみ。
- `promote_authorization_marker.sh` に `promoted.json` の `browser_ledger` 検査を追加。既存の `promote_*`・`release_*` 10 本は全て exit 0（`sh` では pipefail で動かない既存 6 本は `bash` で実行）。

未解決: docs/ops への手順（preflight 葉）、実 agent-browser での実機の台帳生成は未実施（人の手順）。
