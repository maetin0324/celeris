# local workspace の未申告成果物走査

## 実装

local workspace の終端 run は、これまでの workspace 直下などの markdown 探索に加え、その run の `artifacts_dir` を ADR-0067 D3-c の制限で走査する。共有 workspace の成果物 directory は ADR-0036 に従い task 固有であり、兄弟の `.taskd/artifacts/<other-task>/` は探索しない。

`celerisctl workspace backfill-artifacts --config <config.toml> --task <task-id> [--dry-run]` は既存実装が local task にも対応するため、運用手順を [undeclared-local-artifacts.md](../../docs/ops/undeclared-local-artifacts.md) に追加した。

## 検証

- `cargo test -p task-dispatch --lib a_local_shared_workspace_registers_its_artifacts_without_sibling_files`: 成功（1 passed）。task 固有 `summary.md` / `report.json` と workspace 直下 `legacy.md` が登録され、兄弟成果物は登録されない。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: 成功（5028 passed, 13 skipped、doc tests 成功）。最初の標準 TMPDIR での試行は Unix socket `SUN_LEN` 超過で無関係な試験が失敗したため、socket path を短くして再実行した。
- `cargo clippy --workspace -- -D warnings`: 成功。
- `cargo fmt --all -- --check` および `git diff --check`: 成功。

## 既存 manaba task の backfill

Task `01M4GYJ3XGJNWZQDF35F1MDE0H` の本番 DB と共有 workspace に対する backfill はこの coding run では実施していない。本番環境での実施は運用セッションが担当する。手順書に `--dry-run` で `assignments.md`、`summary.md`、`reports/*.md` を確認してから登録し、GET `/tasks/{id}/artifacts` で確認するコマンドを記載した。実行結果は運用セッション後にここへ追記する。
