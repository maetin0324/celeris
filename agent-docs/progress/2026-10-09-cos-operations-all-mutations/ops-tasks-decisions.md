# tasks/decisions 操作登録の進捗

tasks 領域の accept・approve・reject・cancel を監査付き operation として登録した。`gate_action_op` が `/cos/operations` の `cos_operation_apply` transaction 内で状態遷移と `ApprovalDecided` event を書き、cancel 後の browser stop は commit 後に行う。残る tasks の changes integrate・PR merge・execution-plan POST/decompose・rereview・tree/adopt、および decisions の knowledge page PUT・notification read 操作は、共有 transaction 関数が未実装のため PENDING に残した。admin notify/test・reload・replay・promote も未着手。

検証: `cargo test -p task-api --test cos_ops_mutations`（6 passed）、`cargo clippy -p task-api --all-targets -- -D warnings`（成功）、`git diff --check`（成功）。

追加: task rereview も共有関数化し、監査 transaction で `Rereview` transition を適用するようにした。done 状態の reviewer 条件タスクで `done → reviewing` と直接 422 を統合試験で確認。`cargo test -p task-api --test cos_ops_mutations cos_ops_mutations_task_rereview` と `cargo clippy -p task-api --all-targets -- -D warnings` は成功。

追加: tasks の rereview と execution/decompose、decisions の notifications read-all/read を ALLOWED に移した。decompose は task-ops の検証・更新計画を read-only planner として分離し、human handler / CoS operation が共有する。notification read は task-core に caller-owned transaction の bulk update を加え、audit row と既読化を同時 commit する。統合試験は task gate・rereview・decompose・notification read/read-all を直接 422 と applied/audit event/state 更新まで確認。

検証: `cargo test -p task-api --test cos_ops_mutations`（9 passed）、`cargo test -p task-api --test cos_ops_registry`（3 passed）、`cargo clippy -p task-api --all-targets -- -D warnings`（成功）、`git diff --check`（成功）。

再開調査: 残る task routes は、integration/PR merge が git・GitHub の副作用を含み、既存 handler がその副作用と DB 更新を単一 SQLite transaction にしていない。execution-plan POST と tree/adopt は `task_ops::execution::adopt_human_plan` / `task_ops::tree_adopt::adopt` が内部で独自 write transaction を実行し、cos_operations を同じ transaction に入れる API がない。knowledge page PUT は KB git commit と SQLite audit の異なる永続先への更新である。いずれも現行共有関数を表面だけ包むと監査行と領域 write が原子的でなくなるため、この再開では PENDING のままとした。変更なし・検査未実施。

## Run #3 再開: 外部操作監査基盤

commit `01786777` で C 操作の `begin_external` / `finish_external` と起動時の stale pending 回収を追加した。外部操作は先に pending と監査 event/card を保存し、同じ idempotency key の再送では receipt のみを返す。完了結果は pending から applied に確定し、30 分より古い pending は起動時に `needs_remediation` へ移す。migration は追加していない。

検証: `cargo test -p task-core cos_external_`（2 件成功）、`cargo fmt --check`、`git diff --check`。helper はまだ C route に接続していない。tasks の PENDING 4 件（integrate・PR merge・execution-plan POST・tree/adopt）と decisions の knowledge page PUT は残り、ALLOWED への移動、fake GitHub を使った経路試験、skill/ctl/API 文書同期、全体検査も未完了。各 domain write と監査の transaction 境界を共有化してから route を登録する。

Run continuation: `SqliteStore` と `TaskStore` に caller-owned transaction API `execution_plan_adopt_tree_tx` / `tree_adopt_apply_tx` を追加し、既存の独自 transaction wrapper は新 API を利用するよう変更した。`cargo fmt`、`cargo check -p task-core --tests`、`git diff --check` は成功。task-ops の計画準備と transaction 内の適用の分離、API handler/CoS の共有関数化、ALLOWED 移動、経由/直接拒否試験、skill/ctl/API 文書更新は未完了。このため PENDING は維持する。
