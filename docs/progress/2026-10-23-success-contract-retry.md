# 成功契約・人手集計の再監査：第3回検証

---
tasks: [01M5KVHDWYTACKJKBNKWQXGX0M]
---

本体の変更と受け入れ条件別の証拠は [前回報告](../../agent-docs/progress/2026-10-23-success-contract-human-ops.md) に記載した。実装は `d1ef3181`、Jev 試験の修正は `e5fedc86`、今回の webhook 試験修正は `4a0f8f93`。main 反映 commit は未確定（`git merge-base --is-ancestor d1ef3181 main` は exit 1）。

## 今回の修正

決定的検査で偽 webhook の受信数が3件になった原因は、HTTP要求前のEOFを空の通知として記録していたこと。関連タスクの `9481ce03` の同ファイル修正を移植し、受信記録を応答前に確定するようにした。新しい回帰試験は空接続・本文途中の切断を送って無応答を確認し、その後の完全な要求1件だけが記録されることを検証する。製品側の通知数のアサーションは緩めていない。

## 保存データと再確認

原票は `/local/celeris/data/measure/main-3/out/{trials,grades,human-ops}.jsonl`、Bイベントは同rootの `runs/exp-db/records.jsonl`。旧採点定義による再集計は `/local/celeris/data/workspaces/01M5KVHDWYTACKJKBNKWQXGX0M/artifacts/main-3-reaggregate/` に保存済み。今回も `inputs.sha256` の全ハッシュ一致、訂正台帳が元記録をbyte一致のprefixとして保持すること、CSV全行43列を確認した。

Aはexecutor passedだがmodel_failure、Bはexecutor_cancelledで、ともにvalidated successは0/1。共有env-lock修正71秒はsharedに一度だけ計上し、A/Bへの重複配賦を取り消した。元raw・gradeは変更していない。task 1による新定義の再採点ではない。

## 検証

検査ログは `/local/celeris/data/workspaces/01M5KVHDWYTACKJKBNKWQXGX0M/artifacts/attempt3-*.log`。

- `cargo test -p celeris --test credential_wait_notify`: exit 0、2 passed。
- `bash scripts/dev/test-affected.sh`: exit 0、6297 passed / 16 skipped。doc testも正常終了。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決条件と開始可否

集計実装は完成。新K4の採点→集計一致は未確認で、K4 pilotは開始不可。task 1のgrader/archiveとの統合確認を要する。本物のB試行にQuestion/Approval待機が生じたexportは未確認（旧main-3は待機0件）。単体試験・fixtureの成功を実待機の観測済みとは扱わない。Aの待機・自動retryの欠測はnullのまま保持する。主比較のcost scopeはCoS callsとshared overheadを含まない。有料pilot・本測定・本番変更は実施していない。

## 受け入れ検査の再提出（run 01M5M0TR319JS7HBDS6GTEN1H8）

2026-10-23 09:07 UTC の人のコメントに従い、既存成果を維持して検査を再実行した。前回の受け入れ失敗は Jev の rate_limit 試験の既知の揺れで、別タスク 01M5M07Y3X が担当する。この再提出ではコードを変更していない。

worktree に引き継がれた履歴での実装コミットは `9dd77c10`（旧報告の `d1ef3181`）、Jev 試験補修は `801279e0`（同 `e5fedc86`）、webhook 補修は `d7687881`（同 `4a0f8f93`）。検査対象 HEAD は `466b0b8f`。確認時の main は `22711d25` で、`git merge-base --is-ancestor 9dd77c10 main` は exit 1。main 反映コミットはまだ無い。

旧 main-3 原票6ファイルの SHA-256、訂正台帳の元記録 prefix 一致、A/B の completion=false、共有71秒の一度だけの計上、CSV全3行43列を再確認した。保存先と未解決条件は上記のとおり。新K4の採点・集計統合は本ブランチでは未確認であり、pilot 開始不可の結論を維持する。

再検査結果（すべて exit 0）:

- `bash scripts/dev/test-affected.sh`: 6297 passed / 16 skipped、doc test 正常終了。前回失敗した rate_limit 試験も今回の全件実行では合格。
- `cargo clippy --workspace -- -D warnings`: 合格。
- `cargo fmt --all -- --check`: 合格。

ログは `/local/celeris/data/workspaces/01M5KVHDWYTACKJKBNKWQXGX0M/artifacts/recheck-test-affected.log`、同ディレクトリの `recheck-clippy.log`、`recheck-fmt.log`。既知の揺れそのものの解消を、この1回の成功からは主張しない。
