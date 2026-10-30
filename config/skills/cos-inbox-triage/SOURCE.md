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
- 2026-10-28（ADR 2026-10-28-cos-session-policy-key D2）: resolve の `policy_version` を run の `<policy_version>` から取るように §の例と説明を直した。daemon が run に渡した版だけが通る（違えば 422）。判断の基準は変えていないので policy_version は 6 のまま。
- 2026-10-28（ADR 2026-10-28-cos-review-back-thread D3'〜D5'）: §12（定期見直しの run は review thread で走り、伝えることがある
  ときだけ `chat.review_report` で統括 CoS チャットに報告する。この run からは受信箱の件を resolve しない）を足し、mount 先の
  説明を `launch.rs` の現行（統括・案件・review の thread にも mount）に合わせた。受信箱の件の判定基準は変えていないので
  policy_version は 6 のまま（config の `[cos.triage] policy_version` も変えない）。
- 2026-10-29（ADR 2026-10-29-phase-integration-auto-repair）: §2 の追加の規則に、段の統合の依頼は daemon の merge repair WU が
  尽きた後にだけ受信箱に出て CoS は escalate すること、古い依頼は daemon の sweep が自動の解消へ戻すことを足した。段の依頼には
  前から `integrate_task` の選択肢が無く CoS は escalate していたので、判断の基準は変わらない。policy_version は 6 のまま
  （config の `[cos.triage] policy_version` も変えない）。
- 2026-10-29（ADR 2026-10-29-cos-review-rework-reopen）: §2・§3 に定型の代答 (e) `review_rework`（reviewer の技術的な差し戻しで
  failed になった task の開き直し、同じ task で 2 回まで、同じ差し戻し文の繰り返しは不可）と、(e) にせず人へ回す条件を足した。
  policy_version を 7 に上げた。config の `[cos.triage] policy_version` の既定も "7"。
- 2026-10-30（ADR 2026-10-30-cos-plan-approval）: §2・§3 に定型の代答 (f) `plan_approval`（問題の無い計画の承認。replan の版を含む。
  refs に待っている版と前の版の plan id、`plan.changes` に差分、`plan.checked` に確認 7 項目）と、(f) にせず人へ回す条件を足した。
  policy_version を 8 に上げた。config の `[cos.triage] policy_version` の既定も "8"。
