import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import {
  closeReasonLabel,
  ctrlChar,
  SOFT_KEYS,
  type TerminalStatus,
  type TerminalTab,
  terminalErrorLabel,
  terminalWsUrl,
} from "./terminal-model";

// web 端末（ADR 2026-10-27-web-terminal）。xterm.js の端末をタブで並べる。開く前にパスワードの再入力（step-up）を
// 求める。切断したら端末は終わる（再接続しない）。入出力はこの画面と端末の間だけを流れ、記録しない。

const inputClass = "mt-1 block w-full min-h-11 rounded border p-2";

async function readStatus(): Promise<TerminalStatus | null> {
  const res = await fetch("/terminal/status", { credentials: "same-origin" });
  if (!res.ok) return null;
  return (await res.json()) as TerminalStatus;
}

function StepUp({ onDone }: { onDone: () => void }) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  return (
    <form
      className="space-y-2 rounded border border-border p-3"
      aria-label="端末を開く前のパスワード確認"
      onSubmit={(e) => {
        e.preventDefault();
        setPending(true);
        setError(null);
        void fetch("/terminal/step-up", {
          method: "POST",
          credentials: "same-origin",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ password }),
        })
          .then(async (res) => {
            if (res.ok) {
              setPassword("");
              onDone();
              return;
            }
            const body = (await res.json().catch(() => ({}))) as { code?: string };
            setError(body.code === "invalid_password" ? "パスワードが違います。" : terminalErrorLabel(body.code));
          })
          .finally(() => setPending(false));
      }}
    >
      <label className="block text-label text-foreground">
        web のパスワード（端末を開くたびに求めます。一定時間で失効します）
        <input
          type="password"
          autoComplete="current-password"
          className={inputClass}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>
      {error && (
        <p role="alert" className="text-danger">
          {error}
        </p>
      )}
      <Button type="submit" variant="primary" disabled={pending || !password}>
        確認する
      </Button>
    </form>
  );
}

function TerminalView({ tab, visible, onEnded }: { tab: TerminalTab; visible: boolean; onEnded: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const socket = useRef<WebSocket | null>(null);
  const ctrl = useRef(false);
  const [ctrlOn, setCtrlOn] = useState(false);
  const [state, setState] = useState<"connecting" | "open" | "ended">("connecting");
  const [message, setMessage] = useState<string | null>(null);
  // 親の再描画で接続をやり直さないよう、知らせ先は ref で持つ。
  const endedRef = useRef(onEnded);
  endedRef.current = onEnded;

  const send = useCallback((data: string) => {
    const ws = socket.current;
    if (ws?.readyState === WebSocket.OPEN) ws.send(new TextEncoder().encode(data));
  }, []);

  useEffect(() => {
    if (!box.current) return;
    const terminal = new Terminal({ cursorBlink: true, convertEol: false, scrollback: 5000, fontSize: 14 });
    const fitter = new FitAddon();
    terminal.loadAddon(fitter);
    terminal.open(box.current);
    fitter.fit();
    term.current = terminal;
    fit.current = fitter;
    const ws = new WebSocket(terminalWsUrl(window.location, tab.target, terminal.cols, terminal.rows));
    ws.binaryType = "arraybuffer";
    socket.current = ws;
    let ended = false;
    const end = (text: string) => {
      if (ended) return;
      ended = true;
      setState("ended");
      setMessage(text);
      endedRef.current();
    };
    ws.onopen = () => setState("open");
    ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        terminal.write(new Uint8Array(event.data));
        return;
      }
      const msg = JSON.parse(String(event.data)) as { type: string; code?: string; reason?: string };
      if (msg.type === "error") end(terminalErrorLabel(msg.code));
      else if (msg.type === "exit") end(closeReasonLabel(msg.reason));
    };
    ws.onclose = () => end(closeReasonLabel("disconnect"));
    const input = terminal.onData((data) => {
      if (ctrl.current) {
        ctrl.current = false;
        setCtrlOn(false);
        const c = ctrlChar(data);
        if (c) return send(c);
      }
      send(data);
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "resize", cols, rows }));
    });
    const observer = new ResizeObserver(() => fitter.fit());
    observer.observe(box.current);
    return () => {
      observer.disconnect();
      input.dispose();
      resize.dispose();
      ws.close();
      terminal.dispose();
    };
  }, [tab.target, send]);

  useEffect(() => {
    if (visible) fit.current?.fit();
  }, [visible]);

  return (
    <section className={visible ? "space-y-2" : "hidden"} aria-label={`端末 ${tab.label}`}>
      <div className="flex flex-wrap items-center gap-2 text-label text-muted-foreground">
        <span role="status">{state === "connecting" ? "接続中…" : state === "open" ? "接続しています" : message}</span>
      </div>
      <div ref={box} className="h-96 min-w-0 overflow-hidden rounded border border-border" />
      <div className="flex flex-wrap gap-2" role="toolbar" aria-label="補助キー">
        <Button
          variant={ctrlOn ? "primary" : "secondary"}
          aria-pressed={ctrlOn}
          onClick={() => {
            ctrl.current = !ctrl.current;
            setCtrlOn(ctrl.current);
            term.current?.focus();
          }}
        >
          Ctrl
        </Button>
        {SOFT_KEYS.map((key) => (
          <Button
            key={key.label}
            aria-label={key.aria}
            onClick={() => {
              send(key.data);
              term.current?.focus();
            }}
          >
            {key.label}
          </Button>
        ))}
        <Button
          onClick={() => {
            const text = term.current?.getSelection();
            if (text) void navigator.clipboard?.writeText(text);
          }}
        >
          コピー
        </Button>
        <Button
          onClick={() => {
            void navigator.clipboard?.readText().then((text) => term.current?.paste(text));
          }}
        >
          貼り付け
        </Button>
        <Button
          variant="destructive"
          disabled={state === "ended"}
          onClick={() => {
            const ws = socket.current;
            if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "close" }));
            else ws?.close();
          }}
        >
          終了
        </Button>
      </div>
    </section>
  );
}

export function TerminalPanel({ tabs, onCloseTab }: { tabs: TerminalTab[]; onCloseTab: (key: string) => void }) {
  const [status, setStatus] = useState<TerminalStatus | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [ended, setEnded] = useState<Record<string, boolean>>({});
  const refresh = useCallback(() => void readStatus().then(setStatus), []);
  useEffect(() => refresh(), [refresh]);
  useEffect(() => {
    if (tabs.length && !tabs.some((t) => t.key === active)) setActive(tabs[tabs.length - 1]?.key ?? null);
  }, [tabs, active]);
  if (!tabs.length) return null;
  const steppedUp = status?.step_up_expires_at != null && Date.parse(status.step_up_expires_at) > Date.now();
  const activeTab = tabs.find((t) => t.key === active);
  return (
    <section className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4" aria-label="端末">
      <h2 className="text-section font-semibold text-foreground">端末</h2>
      <ul className="list-disc space-y-1 pl-5 text-label text-muted-foreground">
        <li>このマシンの端末は celeris を動かすユーザー（rmaeda）の権限で動きます。sudo も使えます。</li>
        <li>クラスタの端末は既存の ssh 接続を借ります。`ssh -O exit` は worker が使う接続も閉じます。</li>
        <li>
          切断すると端末は終わります（再接続しません）。daemon の再起動では残り、release の昇格（web
          の入れ替え）では終わります。
        </li>
        <li>入出力は記録しません。開始と終了（対象・時刻・接続元）だけを記録します。</li>
      </ul>
      {status && !status.enabled && (
        <p role="alert" className="rounded border border-border bg-danger p-3 text-danger-foreground">
          {terminalErrorLabel(status.reason)}
        </p>
      )}
      {status?.enabled && !steppedUp && <StepUp onDone={refresh} />}
      {status?.enabled && steppedUp && (
        <>
          <div className="flex flex-wrap gap-2" role="tablist" aria-label="開いている端末">
            {tabs.map((tab) => (
              <span key={tab.key} className="inline-flex items-center gap-1">
                <Button
                  role="tab"
                  aria-selected={tab.key === active}
                  variant={tab.key === active ? "primary" : "secondary"}
                  onClick={() => setActive(tab.key)}
                >
                  {tab.label}
                  {ended[tab.key] ? "（終了）" : ""}
                </Button>
                <Button aria-label={`${tab.label} のタブを閉じる`} onClick={() => onCloseTab(tab.key)}>
                  ×
                </Button>
              </span>
            ))}
          </div>
          {tabs.map((tab) => (
            <TerminalView
              key={tab.key}
              tab={tab}
              visible={tab.key === activeTab?.key}
              onEnded={() => setEnded((prev) => (prev[tab.key] ? prev : { ...prev, [tab.key]: true }))}
            />
          ))}
        </>
      )}
    </section>
  );
}
