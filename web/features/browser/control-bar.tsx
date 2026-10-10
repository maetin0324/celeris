import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { cn } from "../../lib/utils";
import { type ControlPhase, controlPhaseText, leaseSecondsRemaining, shouldEmphasizeRenewal } from "./browser-model";
import {
  BrowserGatewayError,
  browserControlQuery,
  type ControlCommand,
  type ControlStatus,
  newControlHolder,
  releaseControl,
  sendControl,
} from "./browser-query";

// D3.3 の control bar。状態を先頭に大きく出し、lease の取得・延長・返却を 1 度に 1 つだけ送る。
// 返却の 3 層: 明示の resume、画面離脱（route 遷移・pagehide）の release beacon、期限切れで paused。

export type ControlIds = { taskId: string; runId: string; sessionId: string; runState?: string };
export type ControlAction = "pause" | "takeover" | "renew" | "resume" | "stop";

export type ControlButtonState = Record<ControlAction, boolean>;

/** phase・lease・認証区間から各ボタンを押せるかを決める（true = 押せる）。 */
export function controlButtons(input: {
  status: ControlStatus | undefined;
  isMine: boolean;
  canOperate: boolean;
  pending: boolean;
  resumeConfirmed: boolean;
}): ControlButtonState {
  const { status, isMine, canOperate, pending, resumeConfirmed } = input;
  const none = { pause: false, takeover: false, renew: false, resume: false, stop: false };
  if (!status || !canOperate || pending || status.auth_section) return none;
  const mine = status.phase === "human_control" && isMine;
  return {
    pause: status.phase === "agent_running",
    takeover: status.phase === "paused",
    renew: mine,
    resume: resumeConfirmed && (status.phase === "paused" || mine),
    stop: status.phase !== "stopped" && (status.phase !== "human_control" || mine),
  };
}

/** 読み上げる文。残り秒は 15 秒刻みに丸め、15 秒ごとにだけ変わるようにする（D3.6）。 */
export function controlAnnouncement(
  phase: ControlPhase | string,
  options: { isMine: boolean; inFlight: number; remainingSeconds: number | null },
): string {
  const remaining =
    options.remainingSeconds === null ? null : Math.ceil(Math.max(0, options.remainingSeconds) / 15) * 15;
  const text = controlPhaseText(phase, { ...options, remainingSeconds: remaining });
  return phase === "human_control" && options.isMine && remaining !== null
    ? text.replace(/残り \d+ 秒/, `残り約 ${remaining} 秒`)
    : text;
}

const ERROR_TEXT: Record<string, string> = {
  version_conflict: "状態が変わりました。最新の状態を確かめてから、もう一度操作してください。",
  auth_section_active: "認証を扱っている間は操作できません。",
  not_lease_holder: "この lease は別のセッションが持っています。",
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  not_owner: "この操作は本人のセッションでだけ行えます。",
  attestation_invalid: "本人の署名を確かめられませんでした。画面を読み込み直してください。",
  attestation_unavailable: "署名鍵が設定されていないため操作できません。",
  celeris_unavailable: "Celeris に接続できません。",
  browser_action_pending: "前の操作がまだ終わっていません。",
};

export function controlErrorText(code: string): string {
  return ERROR_TEXT[code] ?? "操作に失敗しました。状態を確かめてください。";
}

const ACTION_LABEL: Record<ControlAction, string> = {
  pause: "一時停止",
  takeover: "引き継ぐ（60 秒）",
  renew: "延長（60 秒）",
  resume: "エージェントに返す",
  stop: "停止",
};

export type ControlBarViewProps = {
  status: ControlStatus | undefined;
  isMine: boolean;
  canOperate: boolean;
  /** 操作できない理由（本人でない・認証区間など）。null なら出さない。 */
  disabledReason: string | null;
  nowSeconds: number;
  pending: boolean;
  message: { ok: boolean; text: string } | null;
  expired: boolean;
  onCommand: (action: ControlAction) => Promise<void> | void;
};

/** useQuery に依存しない表示本体（vitest で phase・有効/無効・強調を確かめる）。 */
export function ControlBarView(props: ControlBarViewProps) {
  const { status, isMine, canOperate, disabledReason, nowSeconds, pending, message, expired, onCommand } = props;
  const [fresh, setFresh] = useState(false);
  const [origin, setOrigin] = useState(false);
  const freshId = useId();
  const originId = useId();
  const phase = status?.phase;
  const remaining = status ? leaseSecondsRemaining(status.lease_expires_at, nowSeconds) : null;
  const mine = phase === "human_control" && isMine;
  const operating = phase === "human_control";
  const emphasize = mine && shouldEmphasizeRenewal(remaining);
  const enabled = controlButtons({ status, isMine, canOperate, pending, resumeConfirmed: fresh && origin });
  const reason = status?.auth_section ? "認証を扱っている間は、すべての操作を止めています。" : disabledReason;
  const headline = status
    ? controlPhaseText(status.phase, { isMine, inFlight: status.in_flight, remainingSeconds: remaining })
    : "操作状態を確認しています";
  const announcement = status
    ? controlAnnouncement(status.phase, { isMine, inFlight: status.in_flight, remainingSeconds: remaining })
    : "操作状態を確認しています";

  async function run(action: ControlAction) {
    await onCommand(action);
    if (action === "resume") {
      setFresh(false);
      setOrigin(false);
    }
  }

  return (
    <section
      aria-label="操作状態"
      data-testid="browser-control-bar"
      data-phase={phase ?? "unknown"}
      data-operating={operating ? "true" : "false"}
      className={cn(
        "sticky top-0 z-10 flex flex-col gap-3 rounded-lg border-2 p-4",
        operating ? "border-warning-foreground bg-warning text-warning-foreground" : "border-border bg-surface",
      )}
    >
      <p className="text-section font-semibold" data-testid="browser-control-phase">
        {headline}
      </p>
      <p role="status" aria-live="polite" className="sr-only" data-testid="browser-control-announcement">
        {announcement}
      </p>
      {expired ? (
        <p className="text-label" data-testid="browser-control-expired">
          操作の期限が切れたため一時停止しました。エージェントは自動では再開しません。
        </p>
      ) : null}
      {reason ? (
        <p className="text-label" data-testid="browser-control-reason">
          {reason}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={!enabled.pause} onClick={() => void run("pause")}>
          {ACTION_LABEL.pause}
        </Button>
        <Button size="sm" variant="primary" disabled={!enabled.takeover} onClick={() => void run("takeover")}>
          {ACTION_LABEL.takeover}
        </Button>
        <Button
          size="sm"
          variant={emphasize ? "primary" : "secondary"}
          disabled={!enabled.renew}
          data-emphasis={emphasize ? "true" : "false"}
          className={cn(emphasize && "ring-2 ring-ring ring-offset-2")}
          onClick={() => void run("renew")}
        >
          {emphasize ? `${ACTION_LABEL.renew} — 残り ${remaining} 秒` : ACTION_LABEL.renew}
        </Button>
        <Button size="sm" disabled={!enabled.resume} onClick={() => void run("resume")}>
          {ACTION_LABEL.resume}
        </Button>
        <ConfirmDialog
          trigger={
            <Button size="sm" variant="destructive" disabled={!enabled.stop}>
              {ACTION_LABEL.stop}
            </Button>
          }
          title="ブラウザ実行を停止しますか"
          target="このブラウザ実行"
          consequence="ブラウザの操作を止めます。エージェントも人も、この run で操作を続けられなくなります。"
          reversibility="元に戻せません。続けるには task を再実行します。"
          followUp="この画面の状態が「停止」になります。"
          confirmLabel="ブラウザ実行を停止"
          onConfirm={() => run("stop")}
        />
      </div>
      {phase === "paused" || mine ? (
        <fieldset className="flex flex-col gap-1 text-label">
          <legend className="font-medium">エージェントに返す前の確認</legend>
          <label htmlFor={freshId} className="flex min-h-11 items-center gap-2">
            <input
              id={freshId}
              type="checkbox"
              checked={fresh}
              disabled={!canOperate || pending}
              onChange={(event) => setFresh(event.target.checked)}
            />
            画面の最新の状態（fresh snapshot）を確かめた
          </label>
          <label htmlFor={originId} className="flex min-h-11 items-center gap-2">
            <input
              id={originId}
              type="checkbox"
              checked={origin}
              disabled={!canOperate || pending}
              onChange={(event) => setOrigin(event.target.checked)}
            />
            開いているサイトが許可された origin であることを確かめた
          </label>
        </fieldset>
      ) : null}
      {message ? (
        <p role={message.ok ? "status" : "alert"} className="text-label" data-testid="browser-control-message">
          {message.text}
        </p>
      ) : null}
    </section>
  );
}

const leaseKey = (ids: ControlIds) => `celeris.browser.lease.${ids.taskId}.${ids.runId}.${ids.sessionId}`;

function readHolder(ids: ControlIds): string | null {
  try {
    return window.sessionStorage.getItem(leaseKey(ids));
  } catch {
    return null;
  }
}
function writeHolder(ids: ControlIds, holder: string | null): void {
  try {
    if (holder) window.sessionStorage.setItem(leaseKey(ids), holder);
    else window.sessionStorage.removeItem(leaseKey(ids));
  } catch {
    // sessionStorage が使えなくても lease の期限（第 3 層）で paused に落ちる。
  }
}

/** 画面離脱の返却（第 2 層）。beacon は CSRF を form 値で送る（D2.4）。 */
export function sendReleaseBeacon(ids: ControlIds, csrf: string): boolean {
  const path = `/browser/control/${ids.taskId}/${ids.runId}/${ids.sessionId}/release`;
  const body = new URLSearchParams({ csrf });
  if (typeof navigator !== "undefined" && typeof navigator.sendBeacon === "function")
    return navigator.sendBeacon(path, body);
  void releaseControl(ids, csrf).catch(() => undefined);
  return true;
}

function useNowSeconds(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

export function ControlBar({
  ids,
  csrf,
  canOperate,
  disabledReason,
}: {
  ids: ControlIds;
  csrf: string | null;
  canOperate: boolean;
  disabledReason: string | null;
}) {
  const control = useQuery({
    ...browserControlQuery(ids.taskId, ids.runId, ids.sessionId, ids.runState),
    enabled: canOperate,
  });
  const status = control.data?.status;
  const nowSeconds = useNowSeconds();
  const [holder, setHolder] = useState<string | null>(() => readHolder(ids));
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [expired, setExpired] = useState(false);
  const latest = useRef({ ids, csrf, holder });
  latest.current = { ids, csrf, holder };

  // lease が外れた（期限・他の操作）ら覚えている holder を捨てる。期限で落ちたときは paused を明示する。
  // 送信直後の古い status で消さないよう、human_control を一度見た lease だけを対象にする。
  const held = useRef(false);
  useEffect(() => {
    if (!status || !holder) {
      held.current = false;
      return;
    }
    if (status.phase === "human_control") {
      held.current = true;
      return;
    }
    if (!held.current) return;
    held.current = false;
    if (status.phase === "paused") setExpired(true);
    setHolder(null);
    writeHolder(ids, null);
  }, [status, holder, ids]);

  // 第 2 層: route 遷移（unmount）と pagehide で lease を返す。
  useEffect(() => {
    const release = () => {
      const { ids: current, csrf: token, holder: held } = latest.current;
      if (!held || !token) return;
      sendReleaseBeacon(current, token);
      writeHolder(current, null);
      latest.current = { ...latest.current, holder: null };
    };
    window.addEventListener("pagehide", release);
    return () => {
      window.removeEventListener("pagehide", release);
      release();
    };
  }, []);

  async function onCommand(action: ControlAction) {
    if (!status || !csrf) return;
    const nextHolder = action === "takeover" ? newControlHolder() : (holder ?? newControlHolder());
    const command: ControlCommand =
      action === "takeover" || action === "renew"
        ? { kind: action, holder: nextHolder, ttl_secs: 60 }
        : action === "resume"
          ? { kind: "resume", holder: nextHolder, fresh_snapshot: true, policy_origin_ok: true }
          : { kind: action };
    setPending(true);
    setMessage(null);
    try {
      await sendControl(ids, command, status.version, csrf);
      if (action === "takeover" || action === "renew") {
        setHolder(nextHolder);
        writeHolder(ids, nextHolder);
        setExpired(false);
      } else if (action === "resume" || action === "stop") {
        setHolder(null);
        writeHolder(ids, null);
      }
      setMessage({ ok: true, text: `${ACTION_LABEL[action]}を送りました。` });
    } catch (error) {
      const code = error instanceof BrowserGatewayError ? error.code : error instanceof Error ? error.message : "";
      setMessage({ ok: false, text: controlErrorText(code) });
      if (action === "stop") throw new Error(controlErrorText(code));
    } finally {
      setPending(false);
      void control.refetch();
    }
  }

  return (
    <ControlBarView
      status={status}
      isMine={holder !== null}
      canOperate={canOperate && csrf !== null}
      disabledReason={disabledReason ?? (control.isError ? "操作状態を取得できません。" : null)}
      nowSeconds={nowSeconds}
      pending={pending}
      message={message}
      expired={expired}
      onCommand={onCommand}
    />
  );
}
