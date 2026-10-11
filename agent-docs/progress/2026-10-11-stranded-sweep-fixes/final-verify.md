# final-verify — 取り残し検出の不備修正の再検証（na-event 統合後）

- 対象 HEAD: `ca0dd20b`（integrate wu/na-event）。実装は変えていない（検証と記録だけ）。
- 基点: `CELERIS_WU_BASE=af7935e1`。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は exit 0（検証 WU は crates/ web/ を変えていない）。
- 実行環境: `TMPDIR=/tmp`（既定の run の tmp では browser・credentiald の unix socket path 長超過で落ちるため。memory の既知事項）。

## 証拠

| 条件 | 実行したコマンド | exit | 要点 |
|---|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 5178 run / 5178 passed / 13 skipped、doctest 3 passed、JSON 集計 passed 5181・failed 0 |
| stranded 試験 | `TMPDIR=/tmp cargo nextest run --workspace -E 'test(stranded_)'` | 0 | 24 passed（前回 23 から `stranded_audit_*` が増えた） |
| lint | `cargo clippy --workspace -- -D warnings` | 0 | 警告無し（`Finished dev profile`） |
| 書式 | `cargo fmt --all -- --check` | 0 | 差分無し |
| 文書検査 | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 文書検査 | `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (194 files)` |
| 文書検査 | `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |
| 文書検査（補足） | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | `check-doc-layout: ok` |
| 文書検査（補足） | `python3 scripts/dev/check-architecture-map.py` | 0 | `OK: 357 件のパスを確認した` |

test-parallel の stderr には `tests left 6 entries in TMPDIR (removed on exit)` の警告が出るが、exit は 0 で、試験の失敗ではない（既知の挙動）。

## F2 の実装突き合わせ（ADR 付記 F2 と event）

- 決定: `unintegrated` 等から `not_applicable` への遷移は `integration_not_applicable` event を追記する（同じ状態の再 sweep は追記しない）。
- 実装: `crates/task-core/src/model.rs` の `Event::IntegrationNotApplicable`（serde 名 `integration_not_applicable`）、`crates/task-core/src/store/task_integration_state.rs` の upsert が追記。
- 試験: `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`（`crates/task-ops/src/stranded.rs`）が通った（上の stranded 24 passed に含む）。
- 結論: F2 の event は決定と実装が一致する。ADR 付記の「ずれ（event）」は解消済みの記述になっている。

## 未解決（この検証で残るもの）

- ADR 付記 F2 の「ずれ」に残る 2 点は、決定本文と実装の文言の違いで、event の不一致とは別。判断は人に上げる（進捗ファイルの未解決事項を参照）。
  - 表の名前: 決定本文は `task_repo_integration_states`、実装は `task_integrations`。
  - reason 文字列: 決定は `remote repository is not checked by the stranded sweep`、実装は `remote repository: not checked by the sweep`。
- 既定の run の TMPDIR では browser・credentiald の試験が落ちる（既知）。この検証は `TMPDIR=/tmp` で行った。
