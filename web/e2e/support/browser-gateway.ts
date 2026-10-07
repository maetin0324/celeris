import { createHash, generateKeyPairSync } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect, type Server as NetServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import type { Page } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { type BrowserBackendOptions, createFakeDaemon, type FakeDaemonOptions } from "./fake-daemon.mjs";
import { startGateway } from "./gateway";

// browser e2e 用の gateway（ADR 2026-10-05-browser-department-web-live-view D2）。一時 dir に password file・
// 0600 の Ed25519 鍵・owner socket を用意し、偽 daemon（browser backend 付き）と偽 dashboard（loopback の
// http+ws。agent-browser の固定 path だけ）に繋ぐ。本人（owner）は socket で challenge を approve して決める。
// owner は gateway に 1 人だけ。loginAsOwner を別の page で呼ぶと前の owner は失効する。

export const BROWSER_E2E_PASSWORD = "browser-e2e-password";
const WS_MAGIC = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

export type FakeDashboard = {
  authority: string;
  /** 受けた HTTP 要求（path・Host・Origin）。 */
  requests: Array<{ path: string; host: string | undefined; origin: string | undefined }>;
  /** WS で client から届いた text message（gateway が通したもの）。 */
  inputs: unknown[];
  /** 開いている WS の数。 */
  readonly streams: number;
  /** 開いている全 WS へ text message を送る。 */
  broadcast(message: unknown): void;
  close(): Promise<void>;
};

function frame(text: string) {
  const body = Buffer.from(text);
  const head = body.length < 126 ? Buffer.from([0x81, body.length]) : Buffer.from([0x81, 126, 0, 0]);
  if (body.length >= 126) head.writeUInt16BE(body.length, 2);
  return Buffer.concat([head, body]);
}

// client の masked text frame を読む（試験の message は 64 KiB 未満）。
function decode(buffer: Buffer): { text: string | null; opcode: number; used: number } | null {
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < 4) return null;
    length = buffer.readUInt16BE(2);
    offset = 4;
  }
  const masked = (buffer[1] & 0x80) !== 0;
  if (buffer.length < offset + (masked ? 4 : 0) + length) return null;
  const mask = masked ? buffer.subarray(offset, offset + 4) : null;
  if (mask) offset += 4;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
  return { text: opcode === 1 ? payload.toString("utf8") : null, opcode, used: offset + length };
}

/** agent-browser 0.38.1 の dashboard を真似た loopback server。 */
export async function startFakeDashboard(): Promise<FakeDashboard> {
  const requests: FakeDashboard["requests"] = [];
  const inputs: unknown[] = [];
  const sockets = new Set<Socket>();
  const server = http.createServer((req, res) => {
    const pathname = (req.url ?? "/").split("?")[0];
    requests.push({ path: req.url ?? "/", host: req.headers.host, origin: req.headers.origin });
    const send = (type: string, body: string) => {
      res.writeHead(200, { "content-type": type });
      res.end(body);
    };
    if (pathname === "/")
      return send(
        "text/html; charset=utf-8",
        '<!doctype html><html lang="ja"><title>dashboard</title><script src="/_next/static/x.js"></script><main id="fake-dashboard">偽 dashboard</main></html>',
      );
    if (pathname === "/_next/static/x.js") return send("text/javascript", "window.fakeDashboard = true;");
    if (pathname === "/api/sessions") return send("application/json", JSON.stringify([{ port: 9333 }]));
    if (pathname === "/api/chat/status") return send("application/json", JSON.stringify({ enabled: false }));
    const session = /^\/api\/session\/(\d{1,5})\/(status|tabs)$/.exec(pathname);
    if (session)
      return send(
        "application/json",
        JSON.stringify(
          session[2] === "status"
            ? { port: Number(session[1]), connected: true }
            : [{ id: "tab-1", title: "請求書", url: "https://billing.example.com/invoices/new" }],
        ),
      );
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
  });
  server.on("upgrade", (req, socket: Socket) => {
    requests.push({ path: req.url ?? "/", host: req.headers.host, origin: req.headers.origin });
    const key = req.headers["sec-websocket-key"];
    if (!/^\/api\/session\/\d{1,5}\/stream(\?|$)/.test(req.url ?? "") || typeof key !== "string") {
      socket.end("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
      return;
    }
    const accept = createHash("sha1")
      .update(key + WS_MAGIC)
      .digest("base64");
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    sockets.add(socket);
    socket.write(frame(JSON.stringify({ type: "frame", data: "fake-screencast" })));
    let buffer = Buffer.alloc(0);
    socket.on("data", (chunk: Buffer) => {
      buffer = Buffer.concat([buffer, chunk]);
      for (let next = decode(buffer); next; next = decode(buffer)) {
        buffer = buffer.subarray(next.used);
        if (next.opcode === 8) return socket.end();
        if (next.text !== null) {
          try {
            inputs.push(JSON.parse(next.text));
          } catch {
            inputs.push(next.text);
          }
        }
      }
    });
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => sockets.delete(socket));
  });
  server.listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("dashboard did not bind TCP");
  return {
    authority: `127.0.0.1:${address.port}`,
    requests,
    inputs,
    get streams() {
      return sockets.size;
    },
    broadcast(message) {
      for (const socket of sockets) socket.write(frame(JSON.stringify(message)));
    },
    close: () =>
      new Promise<void>((resolve) => {
        for (const socket of sockets) socket.destroy();
        server.close(() => resolve());
        server.closeAllConnections();
      }),
  };
}

export type BrowserGatewayOptions = FakeDaemonOptions & {
  /** 偽 browser backend の options（credentialWait・now）。 */
  backend?: Omit<BrowserBackendOptions, "publicKey">;
  /** false で CELERIS_WEB_LIVE_VIEW_UPSTREAM を渡さない（映像なし・イベントだけの監視）。 */
  liveUpstream?: boolean;
};

export async function startBrowserGateway(options: BrowserGatewayOptions = {}) {
  const { backend = {}, liveUpstream = true, ...daemonOptions } = options;
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-browser-"));
  const passwordFile = path.join(dir, "password");
  const tokenFile = path.join(dir, "token");
  const keyFile = path.join(dir, "attestation.key");
  const ownerSocket = path.join(dir, "owner.sock");
  const keys = generateKeyPairSync("ed25519");
  writeFileSync(passwordFile, `${BROWSER_E2E_PASSWORD}\n`, { mode: 0o600 });
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`, { mode: 0o600 });
  writeFileSync(keyFile, keys.privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
  const daemon = createFakeDaemon({
    profile: "rich",
    ...daemonOptions,
    token: FIXTURE_TOKEN,
    browser: { ...backend, publicKey: keys.publicKey },
  });
  const dashboard = await startFakeDashboard();
  let app: { locals: Record<string, unknown> } | undefined;
  const gateway = await startGateway({
    daemonUrl: await daemon.start(),
    daemonTokenFile: tokenFile,
    passwordFile,
    attestationKeyFile: keyFile,
    ownerSocket,
    liveUpstream: liveUpstream ? dashboard.authority : "",
    failedLoginDelayMs: 0,
    registerRoutes: (registered: { locals: Record<string, unknown> }) => {
      app = registered;
    },
  });
  if (!app) throw new Error("gateway app was not registered");
  const locals = app.locals as {
    browserLive: { startSocket(): NetServer | null };
    browserLiveUpgrade: (...args: unknown[]) => void;
  };
  gateway.server.on("upgrade", locals.browserLiveUpgrade);
  const owner = locals.browserLive.startSocket();
  if (!owner) throw new Error("owner socket is not configured");
  await new Promise<void>((resolve) => owner.once("listening", resolve));

  /** `celerisctl browser owner-session approve <challenge>` と同じ要求を owner socket に送る。 */
  function approveChallenge(challenge: string) {
    return new Promise<{ ok: boolean; code: string }>((resolve, reject) => {
      const socket = connect(ownerSocket);
      let reply = "";
      socket.once("connect", () => socket.write(`${JSON.stringify({ op: "approve", challenge })}\n`));
      socket.on("data", (chunk) => {
        reply += chunk;
      });
      socket.once("end", () => resolve(JSON.parse(reply)));
      socket.once("error", reject);
    });
  }
  /** page の context で password login する（cookie は page と共有）。 */
  async function login(page: Page) {
    const response = await page.request.post(`${gateway.base}/login`, {
      headers: { Origin: gateway.base, Accept: "application/json" },
      data: { password: BROWSER_E2E_PASSWORD },
    });
    if (!response.ok()) throw new Error(`login failed: ${response.status()}`);
  }
  /** login して本人（owner）にする。返り値は変更系に要る CSRF token。 */
  async function loginAsOwner(page: Page) {
    await login(page);
    const issued = await page.request.post(`${gateway.base}/browser/owner-session`, {
      headers: { Origin: gateway.base },
    });
    const { challenge } = (await issued.json()) as { challenge: string };
    const approved = await approveChallenge(challenge);
    if (!approved.ok) throw new Error(`owner approve failed: ${approved.code}`);
    const session = (await (await page.request.get(`${gateway.base}/browser/owner-session`)).json()) as {
      isOwner: boolean;
      csrfToken: string | null;
    };
    if (!session.isOwner || !session.csrfToken) throw new Error("owner session was not granted");
    return { csrf: session.csrfToken };
  }
  /** login だけして本人にはしない（not_owner の session）。 */
  async function loginAsOther(page: Page) {
    await login(page);
  }

  return {
    base: gateway.base,
    daemon,
    dashboard,
    publicKey: keys.publicKey,
    approveChallenge,
    loginAsOwner,
    loginAsOther,
    async close() {
      await new Promise<void>((resolve) => owner.close(() => resolve()));
      await gateway.close();
      await dashboard.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
