import type { BrowserRun, BrowserWait } from "../../api/generated/types";

export type ControlPhase = "agent_running" | "pausing" | "paused" | "human_control" | "stopped";
export type LiveUnavailableReason =
  | "owner_unavailable"
  | "not_owner"
  | "not_running"
  | "not_configured"
  | "relay_unavailable"
  | "grant_expired"
  | "launcher_protocol_no_live_frames"
  | "attestation_unavailable"
  | "live_view_guard_unavailable"
  | "live_view_disabled"
  | "not_owner_session"
  | "origin_mismatch"
  | "other_task"
  | "run_ended"
  | "observation_stopped"
  | "grant_denied";

export const liveUnavailableText: Record<LiveUnavailableReason, string> = {
  owner_unavailable: "本人確認を利用できないため、Live View を開けません。",
  not_owner: "本人として登録したセッションで開いてください。",
  not_running: "ブラウザ実行中のみ Live View を利用できます。",
  not_configured: "Live View が設定されていません。",
  relay_unavailable: "Live View の中継を利用できません。イベントで監視してください。",
  grant_expired: "Live View の許可が期限切れです。画面を更新してください。",
  launcher_protocol_no_live_frames: "launcher が Live View の映像を提供していません。launcher を更新してください。",
  // gateway が frame 経路の grant・guard で拒まれた理由（code は data-reason にも出る）。
  attestation_unavailable: "web の本人署名鍵を利用できないため、Live View を開けません。",
  live_view_guard_unavailable: "Live View の確認に Celeris から応答がありません。画面を更新してください。",
  live_view_disabled: "Celeris で Live View が無効です（本人署名の公開鍵が未設定）。",
  not_owner_session: "本人の署名を Celeris が確かめられませんでした。",
  origin_mismatch: "接続元が一致しないため、Live View を開けません。",
  other_task: "この run の Live View ではありません。",
  run_ended: "この run は終了しています。",
  observation_stopped: "認証区間のため観測を止めています。",
  grant_denied: "Celeris が Live View の許可を出しませんでした。",
};

/** gateway が発行した同一 origin の live path だけを通す。 */
export function safeLivePath(href: unknown): string | null {
  return typeof href === "string" && /^\/browser\/live\/[0-9A-Za-z_-]{1,64}\/[0-9A-Za-z_-]{1,64}$/.test(href)
    ? href
    : null;
}

export function leaseSecondsRemaining(expiresAt: number | null | undefined, nowSeconds: number): number | null {
  return expiresAt == null ? null : Math.max(0, Math.ceil(expiresAt - nowSeconds));
}

export function shouldEmphasizeRenewal(seconds: number | null): boolean {
  return seconds !== null && seconds <= 15;
}

export function controlPhaseText(
  phase: ControlPhase | string,
  options: { isMine?: boolean; inFlight?: number; remainingSeconds?: number | null } = {},
): string {
  switch (phase) {
    case "agent_running":
      return "監視のみ — エージェントが操作中";
    case "pausing":
      return `一時停止を待っています（実行中 ${options.inFlight ?? 0} 件）`;
    case "paused":
      return "一時停止中 — 引き継げます";
    case "human_control":
      return options.isMine ? `あなたが操作中 — 残り ${options.remainingSeconds ?? 0} 秒` : "他のセッションが操作中";
    case "stopped":
      return "停止";
    default:
      return "状態を確認できません";
  }
}

export function browserWaitKind(wait: Pick<BrowserWait, "reason">): "decision" | "credential" | null {
  if (wait.reason === "waiting_for_approval") return "decision";
  if (wait.reason === "waiting_for_auth") return "credential";
  return null;
}

export function browserRunBadge(run: Pick<BrowserRun, "state">, phase?: ControlPhase | null): string {
  if (phase === "paused") return "一時停止中（人の返却待ち）";
  const labels: Record<BrowserRun["state"], string> = {
    RUNNING: "実行中",
    WAITING_FOR_AUTH: "認証待ち",
    WAITING_FOR_APPROVAL: "承認待ち",
    WAITING_FOR_HUMAN: "人の対応待ち",
    COMPLETED: "終了",
    FAILED: "失敗",
  };
  return labels[run.state] ?? "未確認";
}
