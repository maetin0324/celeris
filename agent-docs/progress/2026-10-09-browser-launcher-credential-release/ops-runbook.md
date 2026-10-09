# launcher credential 解放 runbook

tasks: [01M4G0GPW9XHJ0MC236F1R8FG9]

- `crates/task-worker/scripts/launcher-admission-evidence.sh` に `--help`、`--stutter 3`、`--credential` を追加。
- `--stutter 3` は `CELERIS_LAUNCHER_TESTS=require` で実 session admission を3回走らせ、各回の実行中に試験 process group を SIGSTOP/SIGCONT で2回揺さぶる。
- `--credential` は task-worker の `launcher_credential_` と credentiald の `prod_admission` 実試験を実行する。
- `docs/ops/browser-launcher-admission-evidence-run.md` と `docs/ops/browser-launcher-credential-release.md` を更新/追加。解放文書に root launcher 更新・台帳検査・policy・manaba task・rollback を記載。秘密は含めない。
- 検査: `sh -n ...`、`--help`、対象2文書の `check-doc-links.sh`、WU scope paths はすべて exit 0。
- host 上でのみ成立する launcher/userns 実証、台帳取得、本番操作はこの作業環境では実施していない。
