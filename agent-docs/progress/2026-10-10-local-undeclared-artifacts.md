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

attempt 3 で retry の分離漏れを修正した。workspace 末尾が TaskId なら自分の ID との一致で所有を判定する。別 ID なら `parent_id` がなくても共有扱いで `.taskd/artifacts/<task-id>/` を使う。任意名の単独 workspace は従来の配置を維持する。共有 workspace では全体 Markdown 走査も停止し、元 task の `artifacts/` と workspace 直下のファイルが混ざる経路を閉じた。worker の出力先・run 後の走査・backfill は共通の所有判定を使う。

`celerisctl --db <db> workspace backfill-artifacts --config <config.toml> --task <task-id> [--dry-run]` は既存実装が local task にも対応するため、運用手順を [undeclared-local-artifacts.md](../../docs/ops/undeclared-local-artifacts.md) に追加した。

## 検証

- attempt 1/2 の共有 workspace 試験では兄弟成果物だけを除外していた。attempt 3 では元 task の `artifacts/original.md` と直下 `legacy.md` も除外することを確認する試験に置き換えた。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: 成功（5028 passed, 13 skipped、doc tests 成功）。最初の標準 TMPDIR での試行は Unix socket `SUN_LEN` 超過で無関係な試験が失敗したため、socket path を短くして再実行した。
- `cargo clippy --workspace -- -D warnings`: 成功。
- `cargo fmt --all -- --check` および `git diff --check`: 成功。
- 再検証（attempt 2、2026-10-10）: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` exit 0（5028 passed, 0 failed, 14 ignored）。run の長い TMPDIR では socket path の SUN_LEN 超過で 81 件失敗（変更と無関係、前回と同じ）。`cargo clippy --workspace -- -D warnings` exit 0。

## 既存 manaba task の backfill

- 本番 backfill: 未実施（配送後に運用セッションが docs/ops の手順で dry-run → backfill → GET 確認を行う）。運用セッションの回答（2026-10-10）: 今の本番 celerisctl には新しい走査が無く今実施しても意味が無いため、この coding run では本番 DB に書かない。
- 手順: [docs/ops/undeclared-local-artifacts.md](../../docs/ops/undeclared-local-artifacts.md)（`--dry-run` → 本登録 → `GET /tasks/01M4GYJ3XGJNWZQDF35F1MDE0H/artifacts`）。
- 事前確認（読み取りのみ、2026-10-10）: task `01M4GYJ3XGJNWZQDF35F1MDE0H` の workspace `/local/celeris/data/workspaces/01M4FPA6ADFH639XYP894ES87J/artifacts/` に `assignments.md`・`summary.md`・`reports/*.md` 28 件ほか計 33 file（1 MiB 超 0 件）。当時の実装では retry が所有扱いになり `<workspace>/artifacts` に書いていた。attempt 3 以降は専用ディレクトリが対象になるため、運用セッションが各ファイルの帰属を確認してからコピーする。`checkpoint.json`・`result.json` は除外され、件数は D3-c の上限 64 件内に収まる。

## 未解決事項

- 本番での backfill 実施と GET 確認（運用セッション、配送後）。

## attempt 3 の検証

- 修正前: `cargo test -p task-core --lib a_retry_without_a_parent_does_not_own_the_original_tasks_workspace` exit 101。`!owns_workspace(&retry, &dir)` で失敗し、指摘を再現。
- `TMPDIR=/tmp cargo test -p task-dispatch --lib undeclared_artifacts`: exit 0、20 passed。親付き共有 workspace と親なし retry の worker が書いた md/json の登録、元 task・兄弟の除外、D3-c の深さ・件数・サイズ等の既存試験を含む。
- `TMPDIR=/tmp cargo test -p task-core --lib artifacts::tests`: exit 0、5 passed。親なし retry の元 task/別 retry との保存先分離、任意名 workspace の互換性を確認。
- `TMPDIR=/tmp cargo test -p celerisctl backfill_artifacts`: exit 0、3 passed。retry のレポート 30 件の登録、dry-run でイベントが増えないこと、再実行で重複しないこと、元 task・兄弟が混ざらないことを確認。
- `TMPDIR=/tmp cargo test -p task-api --test files local_retry_undeclared`: exit 0、1 passed。web が使う一覧とダウンロード API で、元 task と retry の同名 `summary.md` が分離され、`declared: false`・存在・SHA-256・本文を取得できることを確認。
- `cargo fmt --all -- --check`、`git diff --check`、`bash scripts/dev/check-doc-links.sh`、`bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0、5032 passed / 0 failed / 14 ignored、doc tests 成功。ログ: run 成果物 `retry-full-tests.log`。
- `cargo clippy --workspace -- -D warnings`: exit 0。ログ: run 成果物 `retry-clippy.log`。
