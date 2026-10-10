---
title: "local workspace の未申告成果物走査"
tasks: [01M4JBJ1XRGAVNXYTT0D6VKJX4]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# local workspace の未申告成果物走査

## 実装

local workspace の終端 run は、これまでの workspace 直下などの markdown 探索に加え、その run の `artifacts_dir` を ADR-0067 D3-c の制限で走査する。共有 workspace の成果物 directory は ADR-0036 に従い task 固有であり、兄弟の `.taskd/artifacts/<other-task>/` は探索しない。

`celerisctl --db <db> workspace backfill-artifacts --config <config.toml> --task <task-id> [--dry-run]` は既存実装が local task にも対応するため、運用手順を [undeclared-local-artifacts.md](../../docs/ops/undeclared-local-artifacts.md) に追加した。

## 検証

- `cargo test -p task-dispatch --lib a_local_shared_workspace_registers_its_artifacts_without_sibling_files`: 成功（1 passed）。task 固有 `summary.md` / `report.json` と workspace 直下 `legacy.md` が登録され、兄弟成果物は登録されない。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: 成功（5028 passed, 13 skipped、doc tests 成功）。最初の標準 TMPDIR での試行は Unix socket `SUN_LEN` 超過で無関係な試験が失敗したため、socket path を短くして再実行した。
- `cargo clippy --workspace -- -D warnings`: 成功。
- `cargo fmt --all -- --check` および `git diff --check`: 成功。
- 再検証（attempt 2、2026-10-10）: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` exit 0（5028 passed, 0 failed, 14 ignored）。run の長い TMPDIR では socket path の SUN_LEN 超過で 81 件失敗（変更と無関係、前回と同じ）。`cargo clippy --workspace -- -D warnings` exit 0。

## 既存 manaba task の backfill

- 本番 backfill: 未実施（配送後に運用セッションが docs/ops の手順で dry-run → backfill → GET 確認を行う）。運用セッションの回答（2026-10-10）: 今の本番 celerisctl には新しい走査が無く今実施しても意味が無いため、この coding run では本番 DB に書かない。
- 手順: [docs/ops/undeclared-local-artifacts.md](../../docs/ops/undeclared-local-artifacts.md)（`--dry-run` → 本登録 → `GET /tasks/01M4GYJ3XGJNWZQDF35F1MDE0H/artifacts`）。
- 事前確認（読み取りのみ、2026-10-10）: task `01M4GYJ3XGJNWZQDF35F1MDE0H` の workspace `/local/celeris/data/workspaces/01M4FPA6ADFH639XYP894ES87J/artifacts/` に `assignments.md`・`summary.md`・`reports/*.md` 28 件ほか計 33 file（1 MiB 超 0 件）。再実行 task は parent を持たないので `owns_workspace` が真で `artifacts_dir_for` は `<workspace>/artifacts`（backfill が走査する場所と一致）。`checkpoint.json`・`result.json` は除外され、件数は D3-c の上限 64 件内に収まる。

## 未解決事項

- 本番での backfill 実施と GET 確認（運用セッション、配送後）。

## 提案

- retry task が元 task の workspace（path = 元 task id）を所有扱いで共有すると、元 task と同じ `artifacts/` に書く。元 task と retry の成果物を分けたいなら、retry 作成時に workspace を新しい task id で切るか、retry を共有扱い（`.taskd/artifacts/<task-id>/`）にする別 task を検討する。
