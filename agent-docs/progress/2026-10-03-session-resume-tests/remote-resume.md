---
task: 01M40XCG2HFSEBH6J01HPGMX1Z
work_unit: remote-resume
status: done
completed: 2026-10-03
base: 041634c51574
---

# session_resume_remote 試験（ADR-0140 D3: ssh remote workspace の continuation）

## やったこと

- `crates/task-dispatch/src/dispatcher/tests/session_resume.rs` に試験 3 件を追加（既存の `ClaudeScriptAdapter`・
  `three_step_task` と同じ three-step 計画・`session_lines`・`session_of` を流用）。Task の workspace は
  `WorkspaceSpec::Remote { cluster: "sirius", path: "/work/x" }`、クラスタは `cluster_spec_with_auth`（rsync）。
  - `session_resume_remote_same_account_and_local_cwd_resumes`: 同じ account・同じ手元 cwd の continuation
    （予算切れ → yield → done）は同じ session id を `resume = true` で受け取る。session 行の `cwd` は手元の写し
    （`workspace_root` の下）で、クラスタの path（`/work/x`）ではない。`surface_unsupported` にならない。
  - `session_resume_remote_other_account_falls_back_to_checkpoint`: 保存 session が別 account（`acct-other`）に
    なったら `account_changed` で checkpoint 前置きの新 session に倒れ、どの run の RunContext でも
    他 account の session id が `resume = true`（`--resume`）で渡らない。`session_lines` にも
    `resumed (session=<他 account の id>)` が無い。次の continuation は fallback の session を resume する。
  - `session_resume_remote_rejected_resume_falls_back_to_checkpoint`: resume が拒否された（session が無い）run の
    後は `resume_rejected` で、直近の checkpoint（`next_action`）を前置きにした新 session で 1 回やり直す。
- 各 run が remote 経路を通ったことは `push_remote_after_run` の進行の行（`pushed the workspace to cluster sirius…`）
  が 3 本以上あることで確かめる。
- 試験の継ぎ目: `Dispatcher::set_cluster_ssh_command_override`（`dispatcher/cluster.rs`）を追加し、remote の
  worker run（`dispatch_run.rs`）と判定（`review_spawn.rs`）が使う `SshSettings` の `ssh_command` /
  `rsync_command` を偽の `true` に差し替える（`remote_ssh_settings`）。本番の配線は呼ばないので挙動は不変。
  master の生存は既存の `set_cluster_liveness_probe` で偽にする。外部ネットワーク・実 ssh・sleep は使わない。
- 実装は ADR-0140 D3（remote worktree は resume できる、account isolation）と一致していたので、判断表の
  修正は無し。ADR-0140 に試験の所在の付記だけ足した。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-dispatch session_resume_remote` | exit 0、`test result: ok. 3 passed; 0 failed`（0.26s） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo test --workspace` | exit 0、全 test result の合計 passed=3616 failed=0 |
| `cargo fmt -p task-dispatch` | 差分は整形済み |

## 未解決事項

- account の違いは既存試験と同じく `Hook::RewriteStored` で保存行を書き換えて再現している（実際のアカウント
  プールの付け替えは通していない）。プール経由の再現は別試験の範囲。
- 手元 cwd が run ごとに変わる remote 構成（D3 の `surface_unsupported`）は、今の dispatcher では写しが
  `workspace_root/<task_id>` 固定なので発生しない。試験は置いていない。

## 提案

- なし。
