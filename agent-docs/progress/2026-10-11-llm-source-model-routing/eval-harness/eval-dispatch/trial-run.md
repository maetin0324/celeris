---
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
---

# trial-run

- Dispatcher に `eval_trials` tick 段を追加。通常 dispatch 後、評価予約枠・source account の同時実行数・quota/cooldown を確認し、固定 harness/source/model の試行を新 worktree・新 session で起動する。
- RunRequest は objective のみを渡し acceptance を空にする。route_features/options/decisions に stage=`eval` と harness version を記録し、runs は role=`eval` とする。ownerless run 回収では tasks 行のない eval を task 照合対象から除外した。
- `eval_trial_reserved_slot_hides_checks_and_records_selected_version` で check command 非表示、予約枠 1 による同時起動抑止、selected.harness_version を確認する。
- 前 run の scope check は開始 snapshot の task-core 3 path を列挙するが、許可正規表現に `crates/task-core/` が無く失敗する。これは作業差分の変更で解消できないため、replan で allowlist に task-core path を追加する必要がある。
