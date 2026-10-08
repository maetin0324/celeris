source: Celeris 自作（外部 skill の写しではない）
origin: [ADR 2026-10-05 cos-chat-home](../../../agent-docs/adr/2026-10-05-cos-chat-home.md) の D3「一次対応の起動とルーティング」「人に回す基準」「代答・取り消し・差し戻し」「Discord 通知」
written: 2026-10-06
license: このリポジトリと同じ（外部の license は無い）

## Notes

- ADR の D3 が「実行前置き/skill に現行 docs/ops と selfdeploy の検査→promote 手順を渡す」「既定 skill はこの表を含む」と
  決めたので、その内容を CoS 特別 worker に渡す skill として書いた。契約の正本は ADR で、食い違えば ADR が優先する。
- ADR を改訂したら、この skill の該当節と `metadata.version`（policy_version）を一緒に直す。
- 進捗: [agent-docs/progress/2026-10-06-cos-run/prompt-skill.md](../../../agent-docs/progress/2026-10-06-cos-run/prompt-skill.md)
- 2026-10-08（ADR 2026-10-08-cos-chat-prompt-cache 付記 D7）: 受信箱スレッドの run と、受信箱の件を渡された run にだけ
  mount するようにした（`task-dispatch` の `cos_chat_skills`）。通常のスレッドには載らない。policy（version 2）は変えていない。
  cos-operator の §5・§10 への参照を production.md・explaining.md に直した。
