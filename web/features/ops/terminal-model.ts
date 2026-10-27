// web 端末の画面の規則（ADR 2026-10-27-web-terminal）。描画に依らない部分だけを置き、単体試験で確かめる。

export type TerminalTarget = "local" | `cluster:${string}`;

export type TerminalTab = { key: string; target: TerminalTarget; label: string };

export type TerminalStatus = {
  enabled: boolean;
  reason: string | null;
  step_up_expires_at: string | null;
  sessions: { id: string; target: string; opened_at: string }[];
  limits: { max_sessions: number; idle_secs: number; max_secs: number; step_up_secs: number };
};

/** 同じ origin の WebSocket の URL（http → ws、https → wss）。 */
export function terminalWsUrl(
  location: { protocol: string; host: string },
  target: TerminalTarget,
  cols: number,
  rows: number,
) {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const query = new URLSearchParams({ target, cols: String(cols), rows: String(rows) });
  return `${scheme}//${location.host}/terminal/ws?${query.toString()}`;
}

const REASONS: Record<string, string> = {
  user: "終了しました",
  disconnect: "接続が切れたので終了しました（再接続はしません）",
  idle_timeout: "入出力が無い時間が上限を超えたので終了しました",
  max_duration: "最大継続時間に達したので終了しました",
  logout: "ログアウトしたので終了しました",
  exited: "シェルが終了しました",
  spawn_failed: "端末を起動できませんでした",
  gateway_shutdown: "web の再起動（release の昇格を含む）で終了しました",
};

/** 端末が終わった理由の表示。 */
export function closeReasonLabel(reason: string | null | undefined): string {
  return (reason && REASONS[reason]) || "終了しました";
}

const ERRORS: Record<string, string> = {
  cluster_not_connected: "このクラスタの ssh 接続（ControlMaster）がありません。先に「接続」で接続してください。",
  work_dir_unset: "このクラスタの作業ディレクトリが設定されていません。作業ディレクトリを設定してください。",
  terminal_disabled: "端末は設定で無効になっています（CELERIS_WEB_TERMINAL=off）。",
  terminal_requires_password: "パスワード認証の無い構成では端末を使えません。",
  terminal_unavailable: "daemon に接続できないので端末を使えません。",
  terminal_entry_not_allowed:
    "この入口（暗号化されない LAN の入口など）からは端末を開けません。tailscale かこのマシンの loopback から開いてください。",
  step_up_required: "端末を開く前にパスワードを入力してください。",
  too_many_terminals: "同時に開ける端末の数の上限に達しています。",
  daemon_unreachable: "daemon に接続できませんでした。",
  spawn_failed: "端末を起動できませんでした。",
};

/** 開けなかった理由の表示。 */
export function terminalErrorLabel(code: string | null | undefined): string {
  return (code && ERRORS[code]) || `端末を開けませんでした（${code ?? "不明"}）。`;
}

/** Ctrl と組み合わせた 1 文字の制御文字（a〜z と @[\]^_）。対象外は null。 */
export function ctrlChar(key: string): string | null {
  if (key.length !== 1) return null;
  const code = key.toUpperCase().charCodeAt(0);
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code - 0x40);
  if (key === "?") return "\x7f";
  return null;
}

/** スマートフォンの補助キーが送る列。 */
export const SOFT_KEYS: { label: string; aria: string; data: string }[] = [
  { label: "Esc", aria: "Esc を送る", data: "\x1b" },
  { label: "Tab", aria: "Tab を送る", data: "\t" },
  { label: "↑", aria: "上矢印を送る", data: "\x1b[A" },
  { label: "↓", aria: "下矢印を送る", data: "\x1b[B" },
  { label: "←", aria: "左矢印を送る", data: "\x1b[D" },
  { label: "→", aria: "右矢印を送る", data: "\x1b[C" },
];

/** 端末の対象から表示名を作る。 */
export function targetLabel(target: TerminalTarget, hostname?: string | null): string {
  if (target === "local") return `このマシン${hostname ? `（${hostname}）` : ""}`;
  return target.slice("cluster:".length);
}

/** タブの key（同じ対象を複数開けるよう連番を付ける）。 */
export function nextTab(tabs: TerminalTab[], target: TerminalTarget, label: string, seq: number): TerminalTab[] {
  return [...tabs, { key: `${target}#${seq}`, target, label }];
}
