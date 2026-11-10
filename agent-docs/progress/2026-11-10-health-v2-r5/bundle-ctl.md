---
title: 健全性v2 R5 bundle-ctl — CLI 試験の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: blocked
updated: 2026-11-10
---
# 健全性v2 R5 bundle-ctl — CLI 試験の束ね

## 実装前 brief
1. 根本原因: target ごとに link と支援 module のコンパイルが必要になっている。
2. 責務 owner: celerisctl/tests の test target 構成。architecture-map の「celerisctl（CLI。migration をしない open_client）」の行。
3. 再利用先: common::celerisctl_path（実行時 CARGO_BIN_EXE_celerisctl を優先）、既存の cli_resolution と各 CLI の契約試験、http_sync fixture。
4. 選択肢: 独立 target の維持と、ADR の分類どおりの束ね。分類済みの9 targetだけを2束に移し、共有支援 module を束ごとに1回宣言する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 元の9 target と各 file の mod common、browser_doctor の重複 fixture 宣言。
7. 不変条件: 試験本体・case数・不変条件の試験・CLI起動先を維持する。cases.py、nextest list、nextest run、束単位の cargo test で確認する。既存 nextest 単独では同一 process の干渉を検出できないため cargo test も行う。

## 変更
- commit 件名: `phase 1: bundle celerisctl integration test targets`
- browser_eval_surface_suite: bin_smoke、browser_doctor、browser_ledger_release、campaign_connected、cli_resolution、mcp_client_scopes、no_migrate。
- unmapped_suite: pipe、skills_import。
- common と HTTP fixture は main.rs で宣言し、元の file は crate の module を参照する。試験本体は変更していない。
- no_migrate は一時 DB の schema と子 CLI のみを操作する。親 process の env/CWD/signal を変更しないため、ADR の分類どおり束ねた。
- terminal_pty は PTY と子 session の終了・signal、worker_run_signal は worker の signal と終了を検証するため独立維持。org_migrate_v2 は数字を含むため移動しない。campaign_export、routing_cli、eval_requests、followups は R6 担当で元の path を維持した。

## 検査
基準は `$CELERIS_WU_BASE`。提出時の HEAD で実行。
- classify.py --verify-leaf bundle-ctl: exit 0、担当9 targetの移動、keep/r6 243 targetの元の配置、error 0。
- cases.py --crate celerisctl --base "$CELERIS_WU_BASE": exit 0、試験関数205→205、表の行118→118、不変条件18（7 file）、removals 0。
- cargo nextest list -p celerisctl --message-format json: 前後ともexit 0、試験205→205、binary19→12（integration target18→11）。
- cargo nextest run -p celerisctl -E 'binary_id(celerisctl::browser_eval_surface_suite) | binary_id(celerisctl::unmapped_suite)': exit 0、35 passed。
- cargo test -p celerisctl --test browser_eval_surface_suite --test unmapped_suite: exit 0、33 + 2 passed。同一processで束ごとの全試験が合格した。
- CLI の起動先は既存の common::celerisctl_path を共有し、cli_resolution の6件を束の両実行方式で通した。runtime mapping 優先、埋込みpath、無効mapping拒否、実CLIのhelpを維持している。

- bash scripts/dev/test-affected.sh --list: exit 0、対象celerisctlのみ。
- bash scripts/dev/test-affected.sh: exit 1、冒頭のhealthでarch-map.wrong_path 3件。cargo試験には未到達。
- bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE": exit 1、verdict=fail、fail_grade=3。
- python3 scripts/dev/check-architecture-map.py: exit 1、bin_smoke・cli_resolution・no_migrate の旧path3件が存在しない。
- cargo clippy --workspace -- -D warnings: exit 0。
- cargo fmt --all -- --check: exit 0。
- B0 slices-check.py --write-set R5: exit 0、write-set R5: ok。
- sh scripts/dev/progress-index.sh --check / sh scripts/dev/check-doc-links.sh: ともにexit 0。

## 未解決事項
health と test-affected が architecture-map の旧参照3件で exit 1。地図を直さないと受け入れ条件2を満たせないが、docs/architecture-map.md はR5のwrite set外。範囲外には変更せず plan_issue を申告する。分類器は brace 形式の参照を拾えておらず、この3 targetをbundle-ctlへ誤分類した可能性がある。検査器の変更もこのWUでは行わない。

完了日は未確定。人またはplannerが、地図と分類を修正できる範囲のtask/WUを用意してから再開する。

## 提案
architecture-map の CLI 行は brace 形式で no_migrate・cli_resolution・bin_smoke の元の path を参照している。inventory はこの3件を bundle-ctl に分類済みであり、今回の移動は承認済みのADRと分類に従った。地図はR5のwrite set外なので変更していない。R6への延期では今回のhealthが通らないため、計画の見直しが必要。地図の参照を同時に修正できる範囲を追加するか、分類とADRを修正してこの3 targetを元のpathに維持する。
