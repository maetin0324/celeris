# ledger-check-cli: release 配置前の適合台帳検査

---
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
status: done
updated: 2026-10-08
---

ADR [2026-10-08-browser-prod-enablement](../../../adr/2026-10-08-browser-prod-enablement.md) D1.2 手順 4・D1.4 の CLI。

- `celerisctl browser ledger check --file <conformance.json> [--release <sha12>] [--agent-browser <path>] [--no-host-probe] [--json]` を追加。
- `task_worker::browser_ledger::ledger_status` が判定し、共通の code・message をそのまま返す。`ok` は exit 0、台帳の不備はすべて exit 3。公開 backend があれば credential backend が空でも成功する。
- `--agent-browser` は版を調べる実行ファイル（既定は PATH の `agent-browser`）。`--no-host-probe` は指定の実行ファイルを含め host probe を完全に省略する。台帳自体の agent-browser 版の比較は行う。
- `--json` は `{ok, code, message, release, agent_browser, backends, credential_backends}` の 1 行。`release` と `agent_browser` は台帳の `generated_for` の値（読み取れなければ null）、backend は certify された ID の配列。`--release` を省略すると release の比較を行わない。
- DB・daemon 接続は不要。生成・selfdeploy 配置・daemon の path 解決は別 WorkUnit の担当。

検証: `crates/celerisctl/tests/browser_ledger_release.rs` の CLI 結合試験で一時台帳による ok・missing・invalid・stale_release（旧台帳を含む）・stale_agent_browser（metadata と results）・no_conformant_backend、JSON の欄・1 行出力・exit 0/3 を確認する。host probe は一時 dir の偽実行ファイルで current・stale・missing を検査する。実 agent-browser・ネットワーク・本番 path は使わない。

検証結果（2026-10-08）:

- `cargo test -p celerisctl browser_ledger_release_`: 7 passed、0 failed（exit 0）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --package celerisctl --check`・`sh scripts/dev/progress-index.sh --check`・`git diff --check`: exit 0。
- `CELERIS_WU_BASE` 基準の範囲検査（`crates/celerisctl/**` とこの進捗ファイルを除外）: exit 0。

初回 cargo は sandbox 内で指定 scratch target の一時 path が read-only となったため、同じ環境値を維持して sandbox 外で実行した。本番 path・daemon・DB は変更していない。selfdeploy の検査・配置試験は後続 WorkUnit に残る。
