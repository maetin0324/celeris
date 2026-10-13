# final-verify — 取り残し検出の不備修正の再検証（na-event 統合後）

- 対象 HEAD: `75ed7cb3`（na-event 統合 `ca0dd20b` の後）。実装コードは変えていない。変えたのは ADR 付記 F2 の決定本文（表の名前と reason 文字列を実装に合わせた、agent-docs/ のみ）。
- 基点: `CELERIS_WU_BASE=af7935e1`。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は exit 0（crates/ web/ 差分ゼロ）。
- 実行環境: `TMPDIR=/tmp`（既定の run の tmp では browser・credentiald の unix socket path 長超過で落ちるため）。

## 証拠

| 条件 | 実行したコマンド | exit | 要点 |
|---|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 174 binary、JSON 集計 `passed: 5181, failed: 0, ignored: 14`、doctest 3 passed（`CELERIS_TEST_SUMMARY` 行）。tmp の残りは 4 件（終了時に削除、exit には無関係） |
| lint | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`（警告無し） |
| 書式 | `cargo fmt --all -- --check` | 0 | 差分無し |
| 文書検査 | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 文書検査 | `sh scripts/dev/check-adr-numbers.sh` | 0 | ok |
| 文書検査 | `sh scripts/dev/progress-index.sh --check` | 0 | ok |
| 文書検査（補足） | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | ok |
| 文書検査（補足） | `python3 scripts/dev/check-architecture-map.py` | 0 | `OK: 357 件のパスを確認した` |
| stranded 試験 | `TMPDIR=/tmp cargo nextest run --workspace -E 'test(stranded_)'` | 0 | 24 passed / 5167 skipped |
| 範囲 | `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` | 0 | 差分ゼロ |

### 不安定だった試験について

- 1 回目の全体試験（前回 attempt の check）は `task-dispatch dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart` の 1 件で exit 100 になった。
- 同じ試験を単独で実行（`cargo nextest run -p task-dispatch -E 'test(an_interrupted_integration_is_redone_idempotently_after_restart)'`）すると exit 0（1 passed）。その後の全体試験（今回の 5181 passed）でも通った。
- 原因は確定していない。再現は 1 回だけで、今回の再実行は全体で通った。この試験が時間依存か負荷依存かは未調査（この検証では実装を変えない方針）。後続で `tokio::time::pause` 等の決定的化を検討する余地がある。

## F2 の実装突き合わせ（ADR 付記 F2 と event）

- 決定: `unintegrated` 等から `not_applicable` への遷移は `integration_not_applicable` event を追記する（同じ状態の再 sweep は追記しない）。
- 実装: `crates/task-core/src/model.rs` の `Event::IntegrationNotApplicable`（serde 名 `integration_not_applicable`、Event の serde tag は `type`）、`crates/task-core/src/store/task_integration_state.rs` の upsert が追記。
- 試験: `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`（`crates/task-ops/src/stranded.rs`）が通った（上の stranded 24 passed に含む）。
- 名前と文字列のずれ 2 点（表の名前・reason の文字列）は、決定本文を実装に合わせて改訂した（ADR 付記 F2 の決定本文の `task_integrations.state`・`remote repository: not checked by the sweep`）。決定の内容（遠隔は ssh せず `not_applicable`、受信箱に出さない）は変えていない。
- 結論: F2 は決定（改訂後の本文）と実装が一致する。event の不一致も文字列の不一致も残らない。

## 本番への移行（sweep の event 件数確認）

- 進捗ファイルの「本番への移行」3 に、初回 sweep 後の `integration_not_applicable` の件数確認（読み取り専用、`sqlite3 -readonly`、`json_extract(json,'$.type')`）を入れた。
- この SQL は scratch の SQLite DB（`events(id, task_id, seq, ts, json)`、`json` に `{"type":"integration_not_applicable",...}`）で件数 1 を返すことを確かめた。本番の DB では実行していない（本番操作は人が行う）。

## 未解決（この検証で残るもの）

- 既定の run の TMPDIR では browser・credentiald の試験が落ちる（既知）。この検証は `TMPDIR=/tmp` で行った。
- 上の一度だけ落ちた試験の再発条件は未調査（後続候補）。
