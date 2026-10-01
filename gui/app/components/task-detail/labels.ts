import type { Action } from "~/celeris/types";

export const ACTION_LABELS: Record<Action, string> = {
  approve: "承認",
  reject: "却下",
  answer: "回答",
  cancel: "取り消し",
  retry: "やり直す",
  // ADR-0044 D1 / D2（Phase 53）。「編集」は概要タブ、「再開」はタイムラインタブに置く。
  edit: "編集",
  reopen: "再開",
  // ADR-0070 D2（Phase 116）。failed の失敗バナーに置く。
  rereview: "再レビュー",
  // celeris ADR-0074 D2.4（Phase F3 途中確認）。実行節の途中報告に 3 つのボタンを置く。
  phase_gate: "途中確認",
  // celeris ADR-0079 D8（Phase R3b / R4b）。承認 / replan / 取り下げは「実行の形」カードと受信箱に置く。
  plan_gate: "計画の承認",
};
