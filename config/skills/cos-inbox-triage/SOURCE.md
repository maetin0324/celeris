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
- 2026-10-27（ADR 2026-10-27-yomiyasu-human-text）: §6 に人が読む文面の規則（yomiyasu の要点）を足し、policy_version を 3 に上げた。config の `[cos.triage] policy_version` の既定も "3"。
- 2026-10-28（ADR 2026-10-28-single-human-window-general-cos）: §10（窓口は統括 CoS 1 本、札、`chat.relay_forward` による取り次ぎ、統括 CoS は案件の件に答えない）を足し、policy_version を 5 に上げた。config の `[cos.triage] policy_version` の既定も "5"。
- 2026-10-28（ADR 2026-10-28-cos-routine-delegated-answers）: §2 の表を `[cos.triage] human_required` の新しい分類に合わせ、
  §3 に定型の代答 4 種類（承認済みの枠の内側・上限の一度だけの引き上げ・文書化済みの既定・作業方針の補足）と `basis` の形を足した。
  answer は `basis` 必須（API が 422 で拒否する）。policy_version を 6 に上げた（版 5 は同じ日の窓口 ADR が使った）。config の `[cos.triage] policy_version` の既定も "6"。
