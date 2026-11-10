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
4. 選択肢: 独立 target の維持と、ADR の分類どおりの束ね。分類済みの6 targetだけを2束に移し、共有支援 module を束ごとに1回宣言する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 元の6 target と各 file の mod common、browser_doctor の重複 fixture 宣言。
7. 不変条件: 試験本体・case数・不変条件の試験・CLI起動先を維持する。cases.py、nextest list、nextest run、束単位の cargo test で確認する。既存 nextest 単独では同一 process の干渉を検出できないため cargo test も行う。

実装完了日: 2026-11-10。WU完了日は未確定（受け入れ条件2のdoc test待ち）。

## 変更
- commit 件名: `phase 1: bundle celerisctl integration test targets`（前回成果を cherry-pick）。
- commit 件名: `phase 1: preserve referenced CLI test target paths`。
- browser_eval_surface_suite: browser_doctor、browser_ledger_release、campaign_connected、mcp_client_scopes。
- unmapped_suite: pipe、skills_import。
- reclass の分類に合わせ、bin_smoke・cli_resolution・no_migrate は元の path と base の内容へ戻した。
- common と HTTP fixture は main.rs で宣言する。試験本体は変更していない。
- 全6 file は common::celerisctl_path を呼び、実行時 CARGO_BIN_EXE_celerisctl と埋込みの同名変数で既存の binary を解決する。
- terminal_pty と worker_run_signal は PTY・signal の独立性を保ち、org_migrate_v2 は数字を含むため動かさない。他の r6 target も元の path を維持する。

## 検査
基準は `$CELERIS_WU_BASE`。提出時の HEAD で実行。
- 分類検査: exit 0、担当6 target移動、keep/r6 262 target元の配置、error 0。
- cases.py: exit 0、試験関数205→205、表の行118→118、不変条件18（7 file）、removals 0。
- B0 slices-check.py --write-set R5: exit 0。
- `cargo nextest list -p celerisctl --message-format json`: exit 0、205 tests、15 binaries。base の19 binariesから4減（integration target18→14、6 target→2束）。
- `cargo test -p celerisctl --test browser_eval_surface_suite --test unmapped_suite`: exit 0、22＋2 passed。同一processで束ごとの全試験が合格。
- `bash scripts/dev/test-affected.sh --list`: exit 0、対象celerisctlのみ。
- `bash scripts/dev/test-affected.sh`: exit 101。health verdict=pass（fail_grade=0）、health unit 1＋78件とgate7件合格、nextest 205 passed / 15 binaries。doc testでlibrary targetなしのエラー。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo nextest run -p celerisctl -E 'binary_id(celerisctl::browser_eval_surface_suite) | binary_id(celerisctl::unmapped_suite)'`: exit 0、24 passed / 2 binaries。
- `python3 scripts/dev/check-architecture-map.py`: exit 0、990 path確認。
- `classify.py --check inventory.json`: exit 0、352 targets / 20 bundles / error 0。
- `sh scripts/dev/progress-index.sh --check` と `sh scripts/dev/check-doc-links.sh`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: exit 0、変更は担当6 target・2 main.rs・この進捗のみ。
- r6に復元した3 file のbaseとの差分はゼロ。

## 未解決事項
`test-affected` は試験205件に合格した後、`cargo test --doc -p celerisctl` が「no library targets found」で exit 101。
この base には affected-features の変更がまだ入っていない。scripts/dev/test-affected.sh は別 WU の担当なので変更しない。
受け入れ条件2は未達。affected-features の成果を統合後に再実行する必要がある。plan_issue として申告する。

## 提案
無し。
