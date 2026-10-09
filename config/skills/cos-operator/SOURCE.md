source: Celeris 自作（外部 skill の写しではない）
origin: [ADR 2026-10-05 cos-chat-home](../../../agent-docs/adr/2026-10-05-cos-chat-home.md) の D2「継続・キュー・停止」の要約節と D3「全道具・全権限の具体」
written: 2026-10-06
license: このリポジトリと同じ（外部の license は無い）

## Notes

- ADR の D3 が「実行前置き/skill に現行 docs/ops と selfdeploy の検査→promote 手順を渡す」「既定 skill はこの表を含む」と
  決めたので、その内容を CoS 特別 worker に渡す skill として書いた。契約の正本は ADR で、食い違えば ADR が優先する。
- ADR を改訂したら、この skill の該当節と `metadata.version`（policy_version）を一緒に直す。
- 進捗: [agent-docs/progress/2026-10-06-cos-run/prompt-skill.md](../../../agent-docs/progress/2026-10-06-cos-run/prompt-skill.md)
- 2026-10-08: §10「人への説明の書き方」を追加（ADR 2026-10-08-cos-workspace-files-in-chat D4。人の指摘: 手順が
  curl・toml・systemctl で、場所の分からない workspace の file を案内された）。cos-inbox-triage の packet の summary にも同じ規則を足した。
- 2026-10-08（version 3、ADR 2026-10-08-cos-chat-prompt-cache 付記 D7）: 20,158 B の 1 file を、入口の SKILL.md と
  参照 file（operations.md・attachments.md・production.md・explaining.md）に分けた。毎 run 要る最小の規則は
  prompt の Core（`crates/task-worker/src/cos_chat.rs`）に移し、description から「常に読む」を外した。
  operations.md の登録表は `/cos/operations` の `ALLOWED` と一致させ、試験 `cos_operator_skill_table_matches_allowed`
  （task-api）で固定した。未登録の PUT execution-plan・pause/resume・standing-rules は「登録されていない操作」に移した。
  KB へは `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <kb_root>` で
  参照 file ごと取り込む（本番 KB への取り込みは人が行う）。
- 2026-10-09（version 4）: [案件・repo 必須検査の ADR](../../../docs/adr/2026-10-09-cos-task-repository-required.md)
  に従い、operations.md の起票節へ project_id・repos の規則、登録一覧の確認、例、既存 task の修復手順を追加した。
