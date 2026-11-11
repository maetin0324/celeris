---
title: 健全性v2 R5 bundle-ctl — CLI 試験の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 bundle-ctl — CLI 試験の束ね

完了日: 2026-11-11

WorkUnit `bundle-ctl`（段 ctl）。celerisctl の結合試験のうち、`inventory.json` で leaf が `bundle-ctl` の 6 target を 2 つの束（`crates/celerisctl/tests/<bundle>/main.rs`）にした。試験関数の中身は変えていない。基準は `$CELERIS_WU_BASE`（段 bundle の統合の後。affected-features と reclass の成果を含む）。

## 実装前 brief
1. 根本原因: target ごとに link と支援 module のコンパイルが要る。
2. 責務 owner: `crates/celerisctl/tests` の test target の構成。architecture-map の「celerisctl（CLI。migration をしない open_client）」の行。
3. 再利用先: `common::celerisctl_path`（実行時の `CARGO_BIN_EXE_celerisctl` を優先し、無ければ埋込みの値）、既存の CLI の契約試験、`tests/support/http_sync.rs` の fixture。
4. 選択肢: 独立 target のままにする案と、ADR の分類どおりに束ねる案。分類で `bundle-ctl` の 6 target だけを 2 束に移し、共有の支援 module は束ごとに 1 回宣言する案を採った。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 元の 6 target、各 file の `mod common;`、browser_doctor の fixture の重複宣言。
7. 不変条件: 試験本体・case 数・不変条件の試験・CLI の起動先を変えない。`cases.py`、nextest、束ごとの直接実行（同じ process で全試験）、strace の execve で確かめた。

## 変更
- commit 件名: `phase 1: bundle celerisctl integration test targets`（前回の branch から cherry-pick）。
- commit 件名: `phase 1: preserve referenced CLI test target paths`（前回の branch から cherry-pick）。
- commit 件名: `phase 1: bundle-ctl の進捗を統合後の base の検査結果で書き直す`（この file）。
- `browser_eval_surface_suite`: browser_doctor・browser_ledger_release・campaign_connected・mcp_client_scopes。
- `unmapped_suite`: pipe・skills_import。
- `common` と HTTP fixture は `main.rs` で宣言し、各 file は `use crate::common;` で使う。
- r6（地図が参照する）の bin_smoke・campaign_export・cli_resolution・eval_requests・followups・no_migrate・routing_cli の 7 target は元の path のまま。base との差分は無い。
- keep の cos_ops・terminal_pty・worker_run（束の組に 1 つだけ）と worker_run_signal（signal を扱う process global）は独立 target のまま。org_migrate_v2 は file 名に数字があるので動かしていない（keep）。
- 同じ名前の束（`browser_eval_surface_suite`・`unmapped_suite`）は task-api と celeris にもある。下の検査は package を celerisctl に絞って binary を選んだ。

## 検査（提出時の HEAD で実行）
基準は `$CELERIS_WU_BASE`。前回の run の plan_issue の記録は証拠に使っていない。下はすべてこの run で走らせた結果。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf bundle-ctl agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。移った target 6、元の path にある keep・r6 263、error 0。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate celerisctl --base "$CELERIS_WU_BASE"` → exit 0。試験関数 205 → 205、表の行 118 → 118、名前で追った移動 24、不変条件の試験 18（7 file）、removals 0。
- `bash scripts/dev/test-affected.sh` → exit 0、368 秒。進捗を commit した後にも再実行し、exit 0・136 秒で、下の件数は同じだった。
  - 対象 crate は celerisctl。health は `verdict=pass`、`fail_grade=0`、`repo_health_gate: all checks passed`。
  - nextest は `Starting 205 tests across 15 binaries (303 binaries skipped)`、`205 tests run: 205 passed, 0 skipped`。
  - doc test は `cargo test --doc --workspace` で 14 crate、3 passed・2 ignored・0 failed。base の runner では celerisctl（library 無し）で exit 101 だった段で、affected-features の変更で library の無い crate は doc test の対象から外れた。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0、`repo_health_gate: all checks passed`。
- `git diff --name-only "$CELERIS_WU_BASE" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0、`write-set R5: ok`。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は担当の 6 file・2 つの `main.rs`・この進捗だけ。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh` → exit 0。

## CLI の起動先の確認
`cargo nextest list --workspace -E 'package(=celerisctl)' --message-format json` の `rust-build-meta.non-test-binaries` は celerisctl の bin を `debug/celerisctl` に対応させる。nextest は実行時にこの path を `CARGO_BIN_EXE_celerisctl` で渡す。celerisctl の 2 束の binary を直接走らせ、`strace -f -e trace=execve` で起動した CLI を数えた。
- 実行時の変数なし（コンパイル時に埋め込んだ値を使う）: browser_eval_surface_suite は 22 passed で execve 35 回、unmapped_suite は 2 passed で execve 5 回。どれも `$CARGO_TARGET_DIR/debug/celerisctl`。
- 実行時の変数 = `$CARGO_TARGET_DIR/debug/celerisctl`（nextest と同じ）: 同じ件数が合格し、起動先も同じ 1 つの path。
- 実行時の変数 = 存在しない path: 22 failed と 2 failed。全件が `cannot locate celerisctl: CARGO_BIN_EXE_celerisctl (runtime mapping …` で落ちた。束の中の試験は実行時の変数を読み、PATH や別の target を探さない。

## 未解決事項
無し。

## 提案
無し。
