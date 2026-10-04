---
task: 01M40Z14NQKG7D1BVFP53EQC33
work_unit: restart-resume
status: done
completed: 2026-10-03
base: 4e06df868a60
---

# session_resume_after_restart 試験（ADR-0140 D3: daemon 再起動後の continuation）

## やったこと

- `crates/task-dispatch/src/dispatcher/tests/session_resume.rs` に試験 3 件を追加。
  - 再起動の模擬: 1 つ目の daemon は file の SQLite（`SqliteStore::open`）で WU `a` の最初の run を走らせ（yield）、
    `set_accepting_new_work(false)`（ADR-0040 D4 の drain）で手元の run を片付けて Dispatcher・アダプタ・ストアを
    drop する。この時点で `a` は `needs_continuation`、`node_sessions` に行が残る。2 つ目の daemon は同じ DB file を
    開き直し、新しい Dispatcher（注入した試験用時計 `test_now`、停止時間ぶん 1 時間進める）で最後まで走らせる。
  - 偽アダプタ `ClaudeScriptAdapter` に `with_config_dir` を足し、claude と同じく session の jsonl を
    `<config>/projects/<cwd 変換>/<id>.jsonl` に書く。`--resume` で jsonl が無い・JSON でない行があると
    claude の拒否文言（`No conversation found with session ID: …`）を `session_resume_failed` で報告し、結果なしで失敗する。
    dispatcher は jsonl を見ない（D3）ので、(1)(2) は D3 の「resume 拒否として検出」経路で表した。
  - `session_resume_after_restart_missing_session_file_falls_back_to_checkpoint`: jsonl 削除 → 保存 id の resume が
    1 回だけ試され拒否 → `resume_rejected` で直近 checkpoint（`next_action`）前置きの新 session、最後の run は
    `Completed`、Task は Done、失敗 run は拒否の 1 件だけ、`fresh_fallback_by_reason.resume_rejected = 1`、
    session 行は新 id に置き換わり（cwd 同じ）新 session の jsonl がある。celeris は消えた jsonl を作り直さない。
  - `session_resume_after_restart_corrupt_session_file_falls_back_to_checkpoint`: jsonl を不正な中身に → 同上。
    壊れた file は書き換えられない。
  - `session_resume_after_restart_kept_session_file_resumes_same_session`: jsonl が残り account・cwd 同じ →
    同じ session 行・同じ id を `resume = true` で 1 run、拒否・fallback 無し、jsonl に追記されて 2 行。
- 壊れた file で fallback しない欠陥は見つからなかった（実装の修正なし）。ADR-0140 に試験の所在の付記だけ足した。
- 試験ファイルに sleep は無い（`grep -c sleep` = 0）。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-dispatch session_resume_after_restart` | exit 0、`test result: ok. 3 passed; 0 failed`（5 回連続で同じ） |
| `cargo test -p task-dispatch session_resume` | exit 0、`23 passed; 0 failed`（remote 3 件を含む） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p task-dispatch --tests -- -D warnings` | exit 0 |
| `cargo test --workspace` | exit 0、passed=3619 failed=0 |
| `grep -c sleep crates/task-dispatch/src/dispatcher/tests/session_resume.rs` | 0 |

## 未解決事項

- 実 claude が壊れた jsonl を `--resume` したときに拒否文言を出すか（黙って空の会話で始めるか）は未確認。
  後者なら celeris は拒否を検出できず、checkpoint 前置き（resume 時も要約で載る）だけが頼りになる。
  実機での確認は認証のある環境で人に依頼したい。
- `run_until_idle`（tests/mod.rs）自体は tick 間に実時間の待ちを使っている（本ファイルの外。今回は変えていない）。
