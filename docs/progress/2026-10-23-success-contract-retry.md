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
