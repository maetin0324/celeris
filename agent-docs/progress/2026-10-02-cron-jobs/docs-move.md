# ADR-0131 の文書配置変更

tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]

main の ADR-0128 D6 に従い、`docs/adr/0131-cron-jobs.md` を `agent-docs/adr/0131-cron-jobs.md` へ移した。
ADR 番号は維持した。main の ADR-0133、crates の doc comment、API schema が ADR-0131 を参照するためである。
ADR 番号検査の `ALLOWED_OVER_LAST` に本ファイル名を追加し、番号維持の理由と migration 0046 への振り直しを
ADR 末尾の付記に記録した。

## 番号競合の確認

`git for-each-ref --format='%(refname)'` で列挙した全 ref を `git ls-tree -r --name-only <ref>` で走査した。
この実行環境では並列 WorkUnit の ref に同じ `docs/adr/0131-cron-jobs.md` が見えるため、ref 横断の tree には同名同内容の旧配置が複数ある。別名の `0131-*` は見つからなかった。main が既に ADR-0131 D7 として本 ADR を参照するため番号は維持する。

## 相対リンク

`docs/architecture-map.md`、`docs/ops/cron-jobs.md`、`agent-docs/progress/2026-10-02-cron-jobs/dry-run.md`
の参照先を移動後の `agent-docs/adr/` に合わせた。進捗報告にあった architecture map の旧リンク説明も更新した。

## 検査

- `sh scripts/dev/check-doc-links.sh`
- `sh scripts/dev/check-adr-numbers.sh`
- `sh scripts/dev/progress-index.sh --check`

前回は `crates/task-api/src/cron_jobs.rs:1` にある案件内ではなく利用側リポジトリの API 文書参照を、check が live reference と誤認して失敗した。crate は変更せず、check-doc-links.sh の FOREIGN_DOCS にこのソースを加えて文書走査から除外した。
