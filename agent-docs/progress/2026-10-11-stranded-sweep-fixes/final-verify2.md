# final-verify2: API 文書修正後の再検証

対象 HEAD: `ba294feefae43581b30d6aa838530184ce767608`（api-errors 統合後）。この段では実装を変更せず、再検証結果と進捗記録を更新した。

## 検証結果

| 検査 | 結果 | 証拠 |
|---|---:|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | pass (exit 0) | nextest 5206 件 pass・13 件 skip、doctest 3 件 pass・1 件 ignored。JSON 集計: passed 5209、failed 0、ignored 14、nextest/doctest exit 0。 |
| `cargo clippy --workspace -- -D warnings` | pass (exit 0) | Finished、警告なし。 |
| `cargo fmt --all -- --check` | pass (exit 0) | 差分なし。 |
| `sh scripts/dev/check-doc-links.sh` | pass (exit 0) | `check-doc-links: ok` |
| `sh scripts/dev/check-adr-numbers.sh` | pass (exit 0) | `check-adr-numbers: ok (196 files)` |
| `sh scripts/dev/progress-index.sh --check` | pass (exit 0) | `progress-index --check: ok` |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | pass (exit 0) | `check-doc-layout: ok` |
| `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` | pass (exit 0) | `crates/`・`web/` の差分なし。 |

## API エラー契約の照合

`crates/task-api/src/integrations.rs` と `crates/task-api/src/changes.rs` で実装される 409 code の `default_branch_unresolvable` と `checkout_busy` は、`docs/api/v1/api.md` の §1.5 エラーコード表（行 253・254）に記載されている。merge API の表（行 379）にも両 code があり、§3.80・§3.81 に既定 branch 解決のエラー契約がある。

この契約欠落は api-errors 葉の commit `c8309aed`、統合 commit `1e498748` で解消済み。前回未解決としていた「`default_branch_unresolvable` が api.md に無い」は未解決事項から除去した。

## 残る事項

進捗ファイルに残る内容は、一度だけ失敗した試験の原因調査、ADR F1 の表と一部振る舞いの差、空 repos の Remote 記録範囲、test-parallel の slow 表示、mobile overlay の試験範囲である。これらは後続調査・対象外ケース・本番 host の release/migration 確認に関するもので、今回の acceptance を阻む欠陥ではなく、統合を止めない。本番操作は既存の移行節どおり人が release → verify → promote 手順で行う。

## 作業範囲

変更は `agent-docs/progress/2026-10-11-stranded-sweep-fixes.md` とこの記録のみ。`crates/`・`web/` は変更していない。
