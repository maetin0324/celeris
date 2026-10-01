import { useEffect, useRef, useState } from "react";
import type { ControlCommand, ControlStatus } from "~/celeris/browser-control.server";
import type { BrowserRun } from "~/celeris/types";

const MESSAGES: Record<string, string> = {
  version_conflict: "別の操作で状態が変わりました。最新の状態を読み込み直してください。",
  not_converged: "操作中の処理がまだ収束していません。Paused になるまでお待ちください。",
  auth_section_active: "認証を扱う区間では takeover できません。",
  lease_expired: "操作権の期限が切れました。",
  not_lease_holder: "別のセッションが操作権を持っています。",
};
export class ControlActionGate {
  private pending: Promise<void> | null = null;
  run(send: (key: string) => Promise<void>): Promise<void> {
    if (this.pending) return this.pending;
    const key = crypto.randomUUID();
    const attempt = Promise.resolve().then(() => send(key));
    this.pending = attempt.finally(() => {
      this.pending = null;
    });
    return this.pending;
  }
}
export function controlMessage(code: string): string {
  return MESSAGES[code] ?? "操作を完了できませんでした。状態を更新してください。";
}
export function BrowserControl({
  run,
  csrfToken,
  authInterval,
}: {
  run: BrowserRun;
  csrfToken: string | null;
  authInterval: boolean;
}) {
  const [status, setStatus] = useState<ControlStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [seconds, setSeconds] = useState(Math.floor(Date.now() / 1000));
  const gate = useRef(new ControlActionGate());
  const path = `/browser/control/${encodeURIComponent(run.task_id)}/${encodeURIComponent(run.run_id)}/${encodeURIComponent(run.session_id)}`;
  useEffect(() => {
    if (!csrfToken) return;
    let active = true;
    const refresh = async () => {
      try {
        const res = await fetch(path, { cache: "no-store" });
        const body = await res.json();
        if (active && body.ok) setStatus(body.status);
      } catch {
        /* next poll */
      }
    };
    void refresh();
    const timer = setInterval(() => {
      void refresh();
      setSeconds(Math.floor(Date.now() / 1000));
    }, 2000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [path, csrfToken]);
  if (!csrfToken) return <p className="text-sm text-fg-muted">操作は本人のセッションでのみ利用できます。</p>;
  const blocked = pending || authInterval || status?.auth_section === true;
  const send = async (command: ControlCommand) => {
    if (!status || blocked) return;
    return gate.current.run(async (idempotency_key) => {
      setPending(true);
      setError(null);
      try {
        const res = await fetch(path, {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ csrf: csrfToken, command, expected_version: status.version, idempotency_key }),
        });
        const body = await res.json();
        if (body.ok) {
          setStatus(body.status);
          setConfirmed(false);
        } else setError(controlMessage(body.code));
      } catch {
        setError("接続できませんでした。状態を更新してください。");
      } finally {
        setPending(false);
      }
    });
  };
  return (
    <div className="space-y-2" data-testid="browser-control">
      {status ? (
        <p className="text-sm">
          制御:{" "}
          {status.phase === "pausing"
            ? `収束待ち (${status.in_flight} 件)`
            : status.phase === "paused"
              ? "Paused — takeover 可能"
              : status.phase === "human_control"
                ? "takeover 中"
                : status.phase === "stopped"
                  ? "停止"
                  : "Agent 実行中"}
          {status.lease_expires_at ? ` · lease 残り ${Math.max(0, status.lease_expires_at - seconds)} 秒` : ""}
        </p>
      ) : (
        <p className="text-sm">制御状態を読み込み中…</p>
      )}
      {(authInterval || status?.auth_section) && (
        <p role="status" className="text-sm">
          認証を扱う区間では takeover を無効にしています。
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-danger-soft-fg">
          {error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {status?.phase === "agent_running" && (
          <button type="button" disabled={blocked} onClick={() => void send({ kind: "pause" })}>
            Pause
          </button>
        )}
        {status?.phase === "paused" && (
          <button type="button" disabled={blocked} onClick={() => void send({ kind: "takeover", ttl_secs: 60 })}>
            Takeover (60 秒)
          </button>
        )}
        {status?.phase === "human_control" && (
          <>
            <button type="button" disabled={blocked} onClick={() => void send({ kind: "renew", ttl_secs: 60 })}>
              Renew (60 秒)
            </button>
            <label className="text-sm">
              <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} /> 新しい
              snapshot と policy / origin を再確認した
            </label>
            <button
              type="button"
              disabled={blocked || !confirmed}
              onClick={() => void send({ kind: "resume", fresh_snapshot: true, policy_origin_ok: true })}
            >
              Resume
            </button>
          </>
        )}
        {status && status.phase !== "stopped" && (
          <button type="button" disabled={blocked} onClick={() => void send({ kind: "stop" })}>
            Stop
          </button>
        )}
      </div>
    </div>
  );
}
