---
title: クラスタ（remote workspace）の task が作った成果物が task の成果物一覧に出ない問題を直す
tasks: [01M4AEPH3GJQ1P121ZS61043KY]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# クラスタ（remote workspace）の task が作った成果物が task の成果物一覧に出ない問題を直す

設計は [ADR-0067 付記 2026-10-07](../adr/0067-human-deliverables-policy-and-approval-visibility.md)。
人の指摘（2026-10-07）: BenchFS の 4 FS 比較 task（01M49KWT59、cluster sirius）が artifacts を作ったと報告したが、
web の task の成果物に何も出ない（`ArtifactProduced` が 0 件）。

## 原因

- `run_worker`（`crates/task-dispatch/src/dispatcher/worker_task.rs`）の run 後の未申告成果物の走査は
  `outcome.is_ok() && remote.is_none()` のときだけ走り、remote の task では一度も走らなかった。
- 走査は run の終端を見ず、question / wait で終わった run の扱いが決まっていなかった。
- `existing_paths` が path だけの重複判定で、同じ path の新しい版が登録されない。
- 上限 20 件が `read_dir` 順で消費され、raw の json/csv が数千件ある成果物 dir（この task は 15,512 file）では
  `report.md` が落ちる。

## やったこと（1 run、2026-10-07）

- **D3-a 走る条件**: 終端が `Done` / `Question` / `Waiting` のとき走る（`undeclared_artifacts::terminal_wants_scan`）。
  remote の task は worktree と同じ `artifacts_dir` の中だけの走査。`Yielded` / `BudgetExhausted` / `Err` は走らない。
- **D3-b 重複**: `(path, sha256)` の組（`registered_keys`。run 後の履歴を読み直して、その run が申告した分も含める）。
  中身が変われば新しい版として追加登録（events は追記専用のまま）。
- **D3-c 走査**: 拡張子に `svg` `json` を追加、celeris 自身の json（delegate / followups / plan / review /
  knowledge-candidates）を除外、候補を（拡張子クラス → 深さ → path）で決定的に並べ、深さ 4 まで、1 回 64 件
  （`MAX_FILES_IN_DIR`。全体 `*.md` 走査の 20 件・1 MiB は不変）。
- **D3-d 補完**: `celerisctl workspace backfill-artifacts --config <config.toml> [--task <id>] [--dry-run]`
  （`crates/task-dispatch/src/undeclared_artifacts/backfill.rs`、`crates/celerisctl/src/commands/workspace.rs`）。
  `--task` 無しは remote の task 全部。`run_id` は `runs` 索引の最新 → `WorkerStarted` の最後 → 空文字。何度流しても同じ結果。
- **試験**（決定的。偽 ssh と一時 dir、外部ネットワークなし）:
  - `task-dispatch` `undeclared_artifacts::tests`: 既存 6 を `(path, sha)` 鍵に直し、
    `a_changed_file_at_a_registered_path_is_a_new_version` / `registered_keys_cover_declared_and_undeclared_artifacts` /
    `terminal_wants_scan_for_done_question_and_waiting_only` / `in_dir_scan_orders_by_class_then_depth_then_path_and_skips_machine_files` /
    `in_dir_scan_respects_depth_and_count_limits` と `backfill_tests::*` 3 本を追加。
  - `task-dispatch` `dispatcher::tests::undeclared_artifacts_scan`: remote + question で md/png/svg/pdf/csv が登録される、
    wait → question → done で `(path, sha)` の重複が増えず変更だけ新版、yield は登録しない、local worktree + question
    （dispatcher 経路、task は blocked）で登録される。
  - `celerisctl` `workspace::tests`: `backfill_artifacts_registers_remote_mirror_artifacts_once`（dry-run・本番・冪等・`--task`）、
    `backfill_artifacts_with_an_unknown_task_is_an_error`。
  - `task-api` `files::tests::remote_task_artifacts_resolve_under_the_local_mirror`: remote の写しの相対 path が
    `GET /tasks/{id}/artifacts` で `exists: true` / `sha256_matches: Some(true)` になる。

## 証拠コマンドと結果

- `cargo test -p task-dispatch -- undeclared_artifacts git_worktree_task_registers` → 19 passed（exit 0）
- `cargo test -p celerisctl -- workspace::tests` → 5 passed（exit 0）
- `cargo test -p task-api --lib -- files::tests` → 7 passed（exit 0）
- `bash scripts/dev/test-parallel.sh` → exit 0。`4314 tests run: 4314 passed, 13 skipped`、
  `CELERIS_TEST_SUMMARY {"passed": 4314, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "binaries": 142}`
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。
  注: `--all-targets` を付けると main 由来の `crates/task-core/src/model_catalog/tests.rs`（`cloned_ref_to_slice_refs`）で
  落ちるが、gate の command ではなく本 task の範囲外。

## 本番での補完（配送後に運用セッションが行う。本 run では本番に触れていない）

```
# 1. 候補を見る（登録しない）
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts --config ~/.config/celeris/config.toml --dry-run
# 2. BenchFS の 4 FS 比較 task だけ補完する
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts --config ~/.config/celeris/config.toml --task 01M49KWT59XM37ANGCTJ8A4DX5
# 3. remote の task 全部を補完する（何度流しても増えない）
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts --config ~/.config/celeris/config.toml
# 確認: web の task の成果物、または GET /api/v1/tasks/01M49KWT59XM37ANGCTJ8A4DX5/artifacts
```

`--db` の既定（`CELERIS_DB` / config）は本番の運用に合わせる。DB を開く他の celerisctl コマンドと同じで daemon は止めない。

## 未解決事項

- 走査は run の後に同期で走る（sha256 の計算は最大 64 file × 1 MiB）。成果物 dir が極端に大きい task でも
  深さ 4 までしか見ないので `read_dir` の回数は抑えられるが、計測はしていない。
- `GET /tasks/{id}/artifacts/{idx}` の Content-Type の表（閉じた表）は `svg` / `html` を `application/octet-stream` で
  返す（inline で script が動かないように意図した設計）。web では download になる。inline 表示が要るなら別 task。
- BenchFS task の `artifacts/cmp4/final/` には `aggregate.py` / `plot.py` / `verify.py` / `render-local.cjs` もあるが、
  コードは対象外（人が読む成果物の拡張子だけ）。

## 提案

- 人が読む成果物の置き場の指示文（ADR-0036 D3）に「job の raw は `artifacts/` の深い所（5 段以上）か `raw/` に置く」を
  足すと D3-c の深さ規則と噛み合う。
