# readers-docs: WU 検査ログの読者側追随

tasks: [01M4NV0KJQ2A1ZY49CTPBQN1C7]

2026-10-11 の repro-fix で確認した保存先 `<workspace_root>/<task_id>/work-unit-checks/<wu_key>/` に追随した。

- `GET /tasks/{id}/work-units/{wu_id}/check-log` は event の絶対 `log_path` を読み、task directory 配下に限定する検査はない。files API の葉 WU fixture を task workspace root 配下のログに変更した。
- task-ops の view はイベントの path を合成せず、path 配置への依存はない。
- `docs/api/v1/api.md` と `agent-docs/guides/work-unit-checks.md` は保存先を記述し、worker protocol は作業ツリー外の保存と scope check との関係を説明する。
- `agent-docs/adr/2026-10-08-work-unit-scope-snapshot.md` に 2026-10-11 の付記を追加し、旧 `<task_dir>/work-unit-checks/` 経路を削除した決定を記録した。
- `crates/task-worker/src/preamble.rs` と `crates/task-worker/src/claude_code/prompt.rs` の scope check 説明にログ置き場の記述はなく、矛盾しない。

API handler と task-ops view に保存先制約はなく、event の絶対 `log_path` を利用するため実装修正は不要。schema 変更もない。
