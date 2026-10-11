# final-verify2 — API 文書修正（api-errors）後の再検証

- 対象 HEAD: `1e498748`（api-errors 統合 `1e498748` / api-errors 本体 `c8309aed` の後）。実装コードは変えていない（crates/・web/ 差分ゼロ）。
- 基点: `CELERIS_WU_BASE=c6c1125f`。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は exit 0。
- 実行環境: `TMPDIR=/tmp`（既定の run の tmp では browser・credentiald の unix socket path 長超過で落ちるため）。

## 目的

api-errors 葉が `docs/api/v1/api.md` に `default_branch_unresolvable`・`checkout_busy` の 409 行を追加した後、全体検査を再実行し、前回の差し戻し理由（409 code が api.md のエラーコード表に未記載）が解消したことを確かめる。

## 証拠

| 条件 | 実行したコマンド | exit | 要点 |
|---|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 174 binary、JSON 集計 `passed: 5209, failed: 0, ignored: 14`、doctest 0 failed（`CELERIS_TEST_SUMMARY` 行）。nextest 352.5s・doctest 10.7s（jobs 8）。tmp の残りは 4 件（終了時に削除、exit には無関係） |
| lint | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`（警告無し） |
| 書式 | `cargo fmt --all -- --check` | 0 | 差分無し |
| 文書検査 | `sh scripts/dev/check-doc-links.sh` | 0 | ok |
| 文書検査 | `sh scripts/dev/check-adr-numbers.sh` | 0 | ok |
| 文書検査 | `sh scripts/dev/progress-index.sh --check` | 0 | ok |
| 文書検査（補足） | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | ok |
| 文書検査（補足） | `python3 scripts/dev/check-architecture-map.py` | 0 | ok |
| 範囲 | `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` | 0 | 差分ゼロ |

## API エラー契約の確認（前回差し戻しの解消）

- `crates/task-api/src` が返す 409 の code の突き合わせ:
  - `default_branch_unresolvable`: `crates/task-api/src/changes.rs` の `RepoTarget::default_branch()`（`detail` は `"<repo>: <理由>"`）と `crates/task-api/src/integrations.rs` の merge（`resolve_default_branch` の Err）。
  - `checkout_busy`: `crates/task-api/src/integrations.rs` の `invalid_problem`（`StoreError::Invalid` の `checkout_busy:` 接頭辞）と `mutate_simple` の `MergeOutcome::Busy`。
- 両 code が `docs/api/v1/api.md` §1.5 のエラーコード表にあることを確認（api.md 行 253 `default_branch_unresolvable`・行 254 `checkout_busy`。api-errors 葉の commit `c8309aed` で追加）。§2 表 行 75（merge）・行 76〜78（discard/hold/reopen）・§3.80・§3.81 も 409 の列挙と整合する。
- 解消 commit: `c8309aed`（wu/api-errors）、統合 commit `1e498748`。記載箇所: `docs/api/v1/api.md` の §1.5 エラーコード表（`default_branch_busy` の次の 2 行）と §2・§3.80・§3.81。
- 進捗ファイル（2026-10-11-stranded-sweep-fixes.md）の「未解決事項」から『`default_branch_unresolvable`（409）が api.md に無い』を削除した（解消）。

## 本番への移行

- 変更なし。進捗ファイルの「本番への移行」節（release → verify → promote、migration 0070 の適用確認、初回 sweep の 38 行が `not_applicable` に移る確認、event 38 件の確認、再 sweep で item の id・`at` が変わらない確認、rollback）は api-errors 統合後もそのまま有効。api-errors は docs のみの変更なので本番操作に影響しない。

## 未解決（この検証で残るもの）

- 一度だけ落ちた試験 `task-dispatch dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart` の再発条件は未調査（final-verify.md 記録の通り。今回の全体試験では通った）。マージを止める欠陥ではない（後続で決定的化を検討）。
- 既定の run の TMPDIR では browser・credentiald の試験が落ちる（既知）。この検証は `TMPDIR=/tmp` で行った。
- task-api docs と task-dispatch behind_target の解決失敗時の振る舞いが ADR F1 の表と少し違う（ADR 付記に記載。後続の決定事項）。
- test-parallel の要約が nextest の `passed (N slow)` を数え落とす既知の問題（本件の範囲外、未修正）。
- web の mobile 幅の overlay 試験は `not_applicable` の section を含まない（後続候補）。
- 上記はいずれも本 task の acceptance（default branch 一本化・Remote の not_applicable・item 同一性・検査合格）を阻まない。本番への移行は人の操作（release 手順）のみで、コード側のブロック要因は残らない。
