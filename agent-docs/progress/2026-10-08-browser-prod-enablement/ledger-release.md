---
task: browser-prod-enablement
wu: ledger-release
status: done
completed: 2026-10-08
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
---

# ledger-release: 適合台帳の生成・配置を release に組み込む（ADR 2026-10-08-browser-prod-enablement D1）

## したこと
各葉の進捗:
- [ledger-check-cli](ledger-release/ledger-check-cli.md): `celerisctl browser ledger check`（D1.2 の 4・D1.4 の判定）
- [daemon-path](ledger-release/daemon-path.md): daemon が release dir の台帳を `configure_conformance` で worker に渡す（D1.4）
- [selfdeploy-ledger](ledger-release/selfdeploy-ledger.md): `sd_browser_ledger`・release.sh の browser-ledger 段（非 blocking）・promote の記録・`browser-ledger.sh` と試験
- runner-release: `scripts/browser-conformance.py` に `--celeris-release` と `generated_for`（commit bac9ab05。葉の進捗ファイルは無い）

## 証拠（統合後 HEAD）
- `bash scripts/dev/test-parallel.sh` → exit 0、4742 passed、0 failed、14 ignored。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test -p celerisctl browser_ledger_release_` → exit 0、7 passed。
- `cargo test -p celeris browser_ledger_release_` → exit 0、3 passed。
- `sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0。`sh scripts/dev/check-doc-links.sh` は exit 1（`docs/ops/browser-prod.md` 未作成。下の未解決）。
- `sh scripts/selfdeploy/tests/browser_ledger_release_stages.sh` → exit 0（1 本）。未配置・古い version・配置済み・生成器失敗でも release が落ちないことを fake と一時 dir で確認。

## 未解決
- 本番の台帳生成は人が release（または `browser-ledger.sh`）で回す。実 agent-browser・実 LLM での生成は未実施。
- D1.5 の docs/ops 手順 `docs/ops/browser-prod.md` は未作成（この WU の範囲が agent-docs/progress・adr に限られ、preflight 葉の担当）。task-core の `browser_prerequisite.rs`（49・52 行）の案内文が参照しているため、preflight 葉が作るまで `check-doc-links.sh` は 2 件の broken reference で落ちる。台帳の作り直し手順（`browser-ledger.sh` の実行）を最初の節に入れること。
- 実装の不足は見つからなかった。

## 提案
- runner-release の変更は葉の進捗ファイルが無いため、必要なら次の葉で補う。
