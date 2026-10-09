# tasks/decisions 操作登録の進捗

tasks 領域の accept・approve・reject・cancel を監査付き operation として登録した。`gate_action_op` が `/cos/operations` の `cos_operation_apply` transaction 内で状態遷移と `ApprovalDecided` event を書き、cancel 後の browser stop は commit 後に行う。残る tasks の changes integrate・PR merge・execution-plan POST/decompose・rereview・tree/adopt、および decisions の knowledge page PUT・notification read 操作は、共有 transaction 関数が未実装のため PENDING に残した。admin notify/test・reload・replay・promote も未着手。

検証: `cargo test -p task-api --test cos_ops_mutations`（6 passed）、`cargo clippy -p task-api --all-targets -- -D warnings`（成功）、`git diff --check`（成功）。

追加: task rereview も共有関数化し、監査 transaction で `Rereview` transition を適用するようにした。done 状態の reviewer 条件タスクで `done → reviewing` と直接 422 を統合試験で確認。`cargo test -p task-api --test cos_ops_mutations cos_ops_mutations_task_rereview` と `cargo clippy -p task-api --all-targets -- -D warnings` は成功。

追加: tasks の rereview と execution/decompose、decisions の notifications read-all/read を ALLOWED に移した。decompose は task-ops の検証・更新計画を read-only planner として分離し、human handler / CoS operation が共有する。notification read は task-core に caller-owned transaction の bulk update を加え、audit row と既読化を同時 commit する。統合試験は task gate・rereview・decompose・notification read/read-all を直接 422 と applied/audit event/state 更新まで確認。

検証: `cargo test -p task-api --test cos_ops_mutations`（9 passed）、`cargo test -p task-api --test cos_ops_registry`（3 passed）、`cargo clippy -p task-api --all-targets -- -D warnings`（成功）、`git diff --check`（成功）。
