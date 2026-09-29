import type { IncomingMessage, Server, ServerResponse } from "node:http";
import type { Socket } from "node:net";
import type { Duplex } from "node:stream";
import { checkOwner, onOwnerRevoked } from "~/browser-owner.server";
import {
  cachedLiveViewGuard,
  checkLiveGrant,
  type LiveViewBinding,
  type LiveViewUpstream,
  liveViewBinding,
  liveViewClient,
  liveViewPlain,
  liveViewUpstream,
  readLiveEvents,
  relayResponse,
  runLiveViewRoute,
  upstreamGet,
} from "./browser-live.server";
import {
  encodeWsFrame,
  openWsClient,
  validWsKey,
  WS_OP,
  WsDecoder,
  type WsMessage,
  WsProtocolError,
  wsAcceptKey,
  wsClosePayload,
} from "./ws-codec.server";

/**
 * ADR-0080 D6: 固定版 dashboard（agent-browser 0.38.1）の同一 origin・読み取り専用 relay。
 *
 * dashboard（Next.js の static export）は絶対 URL（`/_next/...`、`fetch("/api/...")`、
 * `ws://<location.host>/api/session/<port>/stream`）を使うので、GUI の origin の root で受ける。
 * どの入口も「cookie → owner grant → HTML を開いたときの束縛 → run と namespace の guard」を通す。
 *
 * - relay する（GET だけ）: `/_next/static/*`、`/api/sessions`、`/api/chat/status`、
 *   `/api/session/:port/tabs`、`/api/session/:port/status`、WebSocket `/api/session/:port/stream`
 * - それ以外の `/api/*`・`/_next/*`、GET 以外は `403 live_view_action_denied`（既定拒否。`/api/exec`・`/api/kill`・
 *   `POST /api/sessions`・`/api/chat`・`/api/models` を含む）
 * - 同じ経路を `/browser/live/:taskId/:runId/...` の下でも受ける（prefix の run は束縛と一致すること）
 * - stream の client→upstream は `ack` / `config`（frame の間隔・速度だけ）の JSON だけ転送し、
 *   `input_mouse` / `input_keyboard` / `input_touch` を含むそれ以外は捨てる（接続は切らない）
 * - logout・期限・再登録（owner grant の失効）で束縛と既存の WebSocket を全て閉じる。接続中も ≤5 秒ごとに
 *   owner と run を照合し、RUNNING/active でなくなった・認証区間に入ったら閉じる
 * - ログに出すのは判断のコードだけ（URL・token・cookie・session ID は出さない）
 */

const ID = "[0-9A-Za-z_-]{1,64}";
const PREFIX_RE = new RegExp(`^/browser/live/(${ID})/(${ID})(/.*)?$`);
const STATIC_RE = /^\/_next\/static\/[0-9A-Za-z._~-]+(?:\/[0-9A-Za-z._~-]+)*$/;
const SESSION_READ_RE = /^\/api\/session\/(\d{1,5})\/(tabs|status)$/;
const STREAM_RE = /^\/api\/session\/(\d{1,5})\/stream$/;
/** 読み取りだけの dashboard API（既定拒否の allow list）。 */
const READ_API = new Set(["/api/sessions", "/api/chat/status"]);

export const LIVE_VIEW_REVALIDATE_MS = 5_000;
let revalidateMs = LIVE_VIEW_REVALIDATE_MS;

/** テスト用: 接続中の再照合の間隔（undefined で既定の 5 秒）。 */
export function setLiveViewRevalidateMsForTest(ms: number | undefined): void {
  revalidateMs = ms ?? LIVE_VIEW_REVALIDATE_MS;
}
const CLIENT_MAX_MESSAGE_BYTES = 64 * 1024;
const UPSTREAM_MAX_MESSAGE_BYTES = 32 * 1024 * 1024;

function logDecision(code: string, dropped?: number): void {
  if (process.env.VITEST) return;
  const entry: Record<string, unknown> = { ts: new Date().toISOString(), live_view: code };
  if (dropped !== undefined) entry.dropped = dropped;
  process.stderr.write(`${JSON.stringify(entry)}\n`);
}

type Target =
  | { kind: "entry"; taskId: string; runId: string }
  | { kind: "sub"; sub: string; prefix: { taskId: string; runId: string } | null };

/** relay が受け持つ経路か。受け持たないなら null（React Router へ）。 */
function classify(pathname: string): Target | null {
  const m = PREFIX_RE.exec(pathname);
  if (m) {
    const taskId = m[1] ?? "";
    const runId = m[2] ?? "";
    const rest = m[3];
    if (!rest || rest === "/") return { kind: "entry", taskId, runId };
    return { kind: "sub", sub: rest, prefix: { taskId, runId } };
  }
  if (pathname === "/api" || pathname.startsWith("/api/") || pathname === "/_next" || pathname.startsWith("/_next/")) {
    return { kind: "sub", sub: pathname, prefix: null };
  }
  return null;
}

/** 読み取りで relay してよい upstream の path か（GET のとき）。 */
function readablePath(sub: string): string | null {
  if (STATIC_RE.test(sub) && !sub.split("/").some((seg) => seg === "." || seg === "..")) return sub;
  if (READ_API.has(sub)) return sub;
  const m = SESSION_READ_RE.exec(sub);
  if (m && validPort(m[1])) return sub;
  return null;
}

function validPort(value: string | undefined): boolean {
  const n = Number(value);
  return Number.isInteger(n) && n > 0 && n < 65536;
}

function isRelayNamespace(sub: string): boolean {
  return sub === "/api" || sub.startsWith("/api/") || sub === "/_next" || sub.startsWith("/_next/");
}

/** Node の要求から、guard に渡す Request（cookie だけを持つ）を作る。 */
function guardRequest(req: IncomingMessage, method = req.method ?? "GET"): Request {
  const encrypted = (req.socket as Socket & { encrypted?: boolean }).encrypted === true;
  const host = typeof req.headers.host === "string" && req.headers.host ? req.headers.host : "localhost";
  const headers = new Headers();
  const cookie = req.headers.cookie;
  if (typeof cookie === "string") headers.set("cookie", cookie);
  const upgrade = req.headers.upgrade;
  if (typeof upgrade === "string") headers.set("upgrade", upgrade);
  let url: string;
  try {
    url = new URL(req.url ?? "/", `${encrypted ? "https" : "http"}://${host}`).href;
  } catch {
    url = `${encrypted ? "https" : "http"}://localhost/`;
  }
  return new Request(url, { method: method === "HEAD" ? "GET" : method, headers });
}

type SubDecision =
  | { ok: true; path: string; binding: LiveViewBinding; sessionHash: string }
  | { ok: false; status: number; code: string };

/** `/_next/*`・`/api/*` の判断（owner → 許可 → 束縛 → 束縛の run の guard）。 */
async function decideSub(
  request: Request,
  method: string,
  sub: string,
  prefix: { taskId: string; runId: string } | null,
  stream: boolean,
): Promise<SubDecision> {
  const owner = await checkOwner(request);
  if (!owner.ok) return owner;
  if (!isRelayNamespace(sub)) return { ok: false, status: 404, code: "not_found" };
  let path: string | null = null;
  if (method === "GET") {
    if (stream) {
      const m = STREAM_RE.exec(sub);
      path = m && validPort(m[1]) ? sub : null;
    } else {
      path = readablePath(sub);
    }
  }
  if (!path) return { ok: false, status: 403, code: "live_view_action_denied" };
  const binding = liveViewBinding(owner.sessionHash);
  if (!binding) return { ok: false, status: 404, code: "live_view_not_bound" };
  if (prefix && (prefix.taskId !== binding.taskId || prefix.runId !== binding.runId)) {
    return { ok: false, status: 404, code: "live_view_not_bound" };
  }
  const guard = await cachedLiveViewGuard(binding.taskId, binding.runId);
  if (!guard.ok) return guard;
  const checked = await checkLiveGrant(binding, owner.sessionHash);
  if (!checked.ok) return checked;
  return { ok: true, path, binding, sessionHash: owner.sessionHash };
}

async function writeResponse(res: ServerResponse, response: Response, head: boolean): Promise<void> {
  const body = Buffer.from(await response.arrayBuffer());
  res.statusCode = response.status;
  response.headers.forEach((value, name) => {
    res.setHeader(name, value);
  });
  res.setHeader("Content-Length", String(body.length));
  res.end(head ? undefined : body);
}

/**
 * express の middleware（`server/app.ts` が React Router のハンドラより前に置く。React Router の root middleware は
 * nonce 付き CSP を掛けるので、dashboard の inline script が動かない）。relay が未設定なら何もしない。
 */
export function liveViewRelayMiddleware(
  req: IncomingMessage,
  res: ServerResponse,
  next: (err?: unknown) => void,
): void {
  if (!liveViewUpstream()) {
    next();
    return;
  }
  let pathname: string;
  try {
    pathname = new URL(req.url ?? "/", "http://relay.invalid").pathname;
  } catch {
    next();
    return;
  }
  const target = classify(pathname);
  if (!target) {
    next();
    return;
  }
  handleRelayHttp(req, res, target).catch((err: unknown) => next(err));
}

async function handleRelayHttp(req: IncomingMessage, res: ServerResponse, target: Target) {
  const method = (req.method ?? "GET").toUpperCase();
  const head = method === "HEAD";
  const upstream = liveViewUpstream();
  if (!upstream) {
    await writeResponse(res, liveViewPlain(503, "live_view_relay_unavailable"), head);
    return;
  }
  if (target.kind === "entry") {
    const response = await runLiveViewRoute(liveViewClient(), guardRequest(req), target.taskId, target.runId);
    if (response.status !== 200) logDecision(`entry_${response.status}`);
    await writeResponse(res, response, head);
    return;
  }
  const decision = await decideSub(guardRequest(req), method, target.sub, target.prefix, false);
  if (!decision.ok) {
    logDecision(decision.code);
    await writeResponse(res, liveViewPlain(decision.status, decision.code), head);
    return;
  }
  let response: Response;
  try {
    response = relayResponse(await upstreamGet(upstream, decision.path));
  } catch {
    logDecision("live_view_upstream_unavailable");
    response = liveViewPlain(502, "live_view_upstream_unavailable");
  }
  await writeResponse(res, response, head);
}

// ---- WebSocket ----

/** 転送してよい client→upstream の message（frame の間隔・速度の設定だけ。session の状態は変えない）。 */
export function allowedClientMessage(text: string): string | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const msg = value as Record<string, unknown>;
  const keys = Object.keys(msg);
  if (msg.type === "ack") {
    if (!keys.every((k) => k === "type" || k === "seq")) return null;
    if (msg.seq !== undefined && !(Number.isSafeInteger(msg.seq) && (msg.seq as number) >= 0)) return null;
    return JSON.stringify(msg.seq === undefined ? { type: "ack" } : { type: "ack", seq: msg.seq });
  }
  if (msg.type === "config") {
    if (!keys.every((k) => k === "type" || k === "maxFps" || k === "pacing")) return null;
    const out: Record<string, unknown> = { type: "config" };
    if (msg.maxFps !== undefined) {
      if (!(Number.isInteger(msg.maxFps) && (msg.maxFps as number) >= 0 && (msg.maxFps as number) <= 120)) return null;
      out.maxFps = msg.maxFps;
    }
    if (msg.pacing !== undefined) {
      if (msg.pacing !== "ack" && msg.pacing !== "push") return null;
      out.pacing = msg.pacing;
    }
    return JSON.stringify(out);
  }
  return null;
}

class RelayConnection {
  readonly sessionHash: string;
  readonly taskId: string;
  readonly runId: string;
  readonly #client: Duplex;
  readonly #upstream: Socket;
  readonly #request: Request;
  readonly #clientDecoder: WsDecoder = new WsDecoder({
    expectMasked: true,
    maxMessageBytes: CLIENT_MAX_MESSAGE_BYTES,
  });
  readonly #upstreamDecoder: WsDecoder = new WsDecoder({
    expectMasked: false,
    maxMessageBytes: UPSTREAM_MAX_MESSAGE_BYTES,
  });
  #closed = false;
  #timer: NodeJS.Timeout | null = null;
  /** 捨てた client→upstream の message の数。 */
  dropped = 0;

  constructor(opts: {
    client: Duplex;
    upstream: Socket;
    request: Request;
    sessionHash: string;
    taskId: string;
    runId: string;
  }) {
    this.#client = opts.client;
    this.#upstream = opts.upstream;
    this.#request = opts.request;
    this.sessionHash = opts.sessionHash;
    this.taskId = opts.taskId;
    this.runId = opts.runId;
  }

  start(clientHead: Buffer, upstreamLeftover: Buffer): void {
    this.#client.on("data", (chunk: Buffer) => {
      void this.#onClientData(chunk);
    });
    this.#upstream.on("data", (chunk: Buffer) => {
      void this.#onUpstreamData(chunk).catch(() => this.close(1011));
    });
    this.#client.on("error", () => this.close(1011, true));
    this.#upstream.on("error", () => this.close(1011, true));
    this.#client.on("close", () => this.close(1001, true));
    this.#upstream.on("close", () => this.close(1001, true));
    this.#client.on("end", () => this.close(1000, true));
    this.#upstream.on("end", () => this.close(1001, true));
    this.#timer = setInterval(() => void this.#revalidate(), revalidateMs);
    this.#timer.unref();
    if (clientHead.length > 0) void this.#onClientData(clientHead);
    if (upstreamLeftover.length > 0) {
      void this.#onUpstreamData(upstreamLeftover).catch(() => this.close(1011));
    }
  }

  get closed(): boolean {
    return this.#closed;
  }

  async #revalidate(): Promise<void> {
    if (this.#closed) return;
    try {
      const owner = await checkOwner(this.#request);
      if (this.#closed) return;
      if (!owner.ok || owner.sessionHash !== this.sessionHash) {
        logDecision("ws_closed_owner");
        this.close(1008);
        return;
      }
      const binding = liveViewBinding(this.sessionHash);
      if (!binding || binding.taskId !== this.taskId || binding.runId !== this.runId) {
        logDecision("ws_closed_unbound");
        this.close(1008);
        return;
      }
      const guard = await cachedLiveViewGuard(this.taskId, this.runId);
      if (this.#closed) return;
      if (!guard.ok) {
        logDecision(`ws_closed_${guard.code}`);
        this.close(1008);
        return;
      }
      const checked = await checkLiveGrant(binding, this.sessionHash);
      if (!checked.ok) {
        logDecision(`ws_closed_${checked.code}`);
        this.close(1008);
      }
    } catch {
      this.close(1011);
    }
  }

  async #onClientData(chunk: Buffer): Promise<void> {
    if (this.#closed) return;
    let messages: WsMessage[];
    try {
      messages = this.#clientDecoder.push(chunk);
    } catch (err) {
      this.close(err instanceof WsProtocolError ? err.closeCode : 1002);
      return;
    }
    for (const msg of messages) {
      if (this.#closed) return;
      switch (msg.opcode) {
        case WS_OP.TEXT: {
          const forwarded = allowedClientMessage(msg.payload.toString("utf8"));
          if (forwarded === null) {
            this.dropped++;
            break;
          }
          await this.#revalidate();
          if (this.#closed) return;
          this.#upstream.write(encodeWsFrame(WS_OP.TEXT, Buffer.from(forwarded, "utf8"), true));
          break;
        }
        case WS_OP.BINARY:
          this.dropped++;
          break;
        case WS_OP.PING:
          this.#client.write(encodeWsFrame(WS_OP.PONG, msg.payload, false));
          break;
        case WS_OP.PONG:
          break;
        case WS_OP.CLOSE:
          this.close(1000);
          return;
      }
    }
  }

  async #onUpstreamData(chunk: Buffer): Promise<void> {
    if (this.#closed) return;
    // Do not forward a frame captured after another task entered an auth interval. The timer
    // also closes idle streams, but periodic checks alone could expose frames in between checks.
    this.#upstream.pause();
    await this.#revalidate();
    if (this.#closed) return;
    let messages: WsMessage[];
    try {
      messages = this.#upstreamDecoder.push(chunk);
    } catch {
      this.close(1011);
      return;
    }
    for (const msg of messages) {
      if (this.#closed) return;
      switch (msg.opcode) {
        case WS_OP.TEXT:
        case WS_OP.BINARY:
          this.#client.write(encodeWsFrame(msg.opcode, msg.payload, false));
          break;
        case WS_OP.PING:
          this.#upstream.write(encodeWsFrame(WS_OP.PONG, msg.payload, true));
          break;
        case WS_OP.PONG:
          break;
        case WS_OP.CLOSE:
          this.close(1001);
          return;
      }
    }
    if (this.#client.writableNeedDrain) this.#client.once("drain", () => this.#upstream.resume());
    else this.#upstream.resume();
  }

  /** 両側に close frame を送り、少し待って破棄する。 */
  close(code: number, abrupt = false): void {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = null;
    unregister(this);
    // 捨てた client→upstream の message の数だけ記録する（中身は出さない）
    logDecision(`ws_closed_${code}`, this.dropped);
    const safeCode = code === 1005 || code === 1006 || code === 1015 ? 1000 : code;
    for (const [sock, masked] of [
      [this.#client, false],
      [this.#upstream, true],
    ] as const) {
      if (!abrupt && !sock.destroyed && sock.writable) {
        try {
          sock.end(encodeWsFrame(WS_OP.CLOSE, wsClosePayload(safeCode), masked));
        } catch {
          // 書けなければ破棄するだけ
        }
      } else if (!sock.destroyed && sock.writable) {
        sock.end();
      }
      const t = setTimeout(() => sock.destroy(), 1_000);
      t.unref();
    }
  }
}

const connections = new Map<string, Set<RelayConnection>>();

function register(conn: RelayConnection): void {
  let set = connections.get(conn.sessionHash);
  if (!set) {
    set = new Set();
    connections.set(conn.sessionHash, set);
  }
  set.add(conn);
}

function unregister(conn: RelayConnection): void {
  const set = connections.get(conn.sessionHash);
  if (!set) return;
  set.delete(conn);
  if (set.size === 0) connections.delete(conn.sessionHash);
}

/** 開いている relay の WebSocket の数（テスト・運用確認用）。 */
export function openLiveViewConnections(): number {
  let n = 0;
  for (const set of connections.values()) n += set.size;
  return n;
}

/** 全ての relay の WebSocket を閉じる（owner grant の失効時。1001）。 */
export function closeAllLiveViewConnections(code = 1001): void {
  for (const set of [...connections.values()]) for (const conn of [...set]) conn.close(code);
}

function onRevoked(): void {
  if (openLiveViewConnections() > 0) logDecision("ws_closed_revoked");
  closeAllLiveViewConnections(1001);
}

const STATUS_TEXT: Record<number, string> = {
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  409: "Conflict",
  502: "Bad Gateway",
  503: "Service Unavailable",
};

function rejectUpgrade(socket: Duplex, status: number, code: string): void {
  logDecision(`ws_${code}`);
  const body = `${code}\n`;
  const lines = [
    `HTTP/1.1 ${status} ${STATUS_TEXT[status] ?? "Error"}`,
    "Content-Type: text/plain; charset=utf-8",
    "Cache-Control: no-store",
    "X-Content-Type-Options: nosniff",
    ...(status === 401 ? ["WWW-Authenticate: Cookie"] : []),
    "Connection: close",
    `Content-Length: ${Buffer.byteLength(body)}`,
  ];
  if (!socket.destroyed) {
    socket.end(`${lines.join("\r\n")}\r\n\r\n${body}`);
    const t = setTimeout(() => socket.destroy(), 1_000);
    t.unref();
  }
}

function expectedOrigin(req: IncomingMessage): string | null {
  const host = req.headers.host;
  if (typeof host !== "string" || !host) return null;
  const encrypted = (req.socket as Socket & { encrypted?: boolean }).encrypted === true;
  return `${encrypted ? "https" : "http"}://${host}`;
}

export interface LiveViewUpgradeOptions {
  /** GUI の Host 許可リスト（server.js の Host 検査と同じ規則）。省略すれば検査しない（テスト）。 */
  hostAllowed?: (host: string) => boolean;
}

/** `upgrade` の 1 件を処理する（relay の経路でなければ 404 で閉じる）。 */
export async function handleLiveViewUpgrade(
  req: IncomingMessage,
  socket: Duplex,
  head: Buffer,
  opts: LiveViewUpgradeOptions = {},
): Promise<void> {
  socket.on("error", () => socket.destroy());
  let pathname: string;
  try {
    pathname = new URL(req.url ?? "/", "http://relay.invalid").pathname;
  } catch {
    rejectUpgrade(socket, 400, "bad_request");
    return;
  }
  const target = classify(pathname);
  if (target?.kind !== "sub") {
    rejectUpgrade(socket, 404, "not_found");
    return;
  }
  const host = req.headers.host;
  if (opts.hostAllowed && (typeof host !== "string" || !opts.hostAllowed(host))) {
    rejectUpgrade(socket, 400, "host_not_allowed");
    return;
  }
  const upstream = liveViewUpstream();
  if (!upstream) {
    rejectUpgrade(socket, 503, "live_view_relay_unavailable");
    return;
  }
  const key = req.headers["sec-websocket-key"];
  if (
    (req.method ?? "").toUpperCase() !== "GET" ||
    (req.headers.upgrade ?? "").toLowerCase() !== "websocket" ||
    req.headers["sec-websocket-version"] !== "13" ||
    !validWsKey(key)
  ) {
    rejectUpgrade(socket, 400, "bad_upgrade");
    return;
  }
  const request = guardRequest(req, "GET");
  // cookie の owner を先に見る（未認証 401 / 他 session 403）。Origin は本人の同一 origin の画面だけ。
  const decision = await decideSub(request, "GET", target.sub, target.prefix, true);
  if (!decision.ok) {
    rejectUpgrade(socket, decision.status, decision.code);
    return;
  }
  const origin = req.headers.origin;
  if (typeof origin !== "string" || origin !== expectedOrigin(req)) {
    rejectUpgrade(socket, 403, "origin_mismatch");
    return;
  }
  await openRelay(req, socket, head, upstream, decision, request, key);
}

async function openRelay(
  _req: IncomingMessage,
  socket: Duplex,
  head: Buffer,
  upstream: LiveViewUpstream,
  decision: Extract<SubDecision, { ok: true }>,
  request: Request,
  key: string,
): Promise<void> {
  let up: Awaited<ReturnType<typeof openWsClient>>;
  try {
    up = await openWsClient({
      host: upstream.host,
      port: upstream.port,
      path: decision.path,
      hostHeader: upstream.authority,
      headers: { Origin: `http://${upstream.authority}` },
    });
  } catch {
    rejectUpgrade(socket, 502, "live_view_upstream_unavailable");
    return;
  }
  const owner = await checkOwner(request);
  if (!owner.ok || owner.sessionHash !== decision.sessionHash) {
    up.socket.destroy();
    rejectUpgrade(socket, owner.ok ? 403 : owner.status, "owner_revoked");
    return;
  }
  // handshake の間に失効していないか（失効の通知で束縛は消える）
  const binding = liveViewBinding(decision.sessionHash);
  if (
    socket.destroyed ||
    !binding ||
    binding.taskId !== decision.binding.taskId ||
    binding.runId !== decision.binding.runId
  ) {
    up.socket.destroy();
    rejectUpgrade(socket, 404, "live_view_not_bound");
    return;
  }
  const checked = await checkLiveGrant(binding, decision.sessionHash);
  if (!checked.ok) {
    up.socket.destroy();
    rejectUpgrade(socket, checked.status, checked.code);
    return;
  }
  const lastSeenRaw = new URL(_req.url ?? "/", "http://relay.invalid").searchParams.get("last_seen");
  const lastSeen = lastSeenRaw !== null && /^\d{1,15}$/.test(lastSeenRaw) ? Number(lastSeenRaw) : 0;
  const page = await readLiveEvents(binding, decision.sessionHash, lastSeen);
  if (!page) {
    up.socket.destroy();
    rejectUpgrade(socket, 503, "live_view_guard_unavailable");
    return;
  }
  onOwnerRevoked(onRevoked);
  socket.write(
    [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${wsAcceptKey(key)}`,
      "",
      "",
    ].join("\r\n"),
  );
  const conn = new RelayConnection({
    client: socket,
    upstream: up.socket,
    request,
    sessionHash: decision.sessionHash,
    taskId: decision.binding.taskId,
    runId: decision.binding.runId,
  });
  register(conn);
  logDecision("ws_opened");
  conn.start(head, up.leftover);
  if (page.plan.kind === "reset") {
    socket.write(
      encodeWsFrame(WS_OP.TEXT, Buffer.from(JSON.stringify({ type: "live_reset", latest_seq: page.plan.latest_seq }))),
    );
    const port = decision.path.split("/")[3];
    for (const path of [`/api/session/${port}/status`, `/api/session/${port}/tabs`]) {
      try {
        const snapshot = await upstreamGet(upstream, path);
        if (snapshot.status === 200)
          socket.write(
            encodeWsFrame(
              WS_OP.TEXT,
              Buffer.from(
                JSON.stringify({
                  type: "live_snapshot",
                  path: path.endsWith("/tabs") ? "tabs" : "status",
                  value: JSON.parse(snapshot.body.toString("utf8")),
                }),
              ),
            ),
          );
      } catch {
        /* A missing dashboard snapshot does not authorize any other data. */
      }
    }
  } else {
    for (const event of page.events)
      socket.write(
        encodeWsFrame(
          WS_OP.TEXT,
          Buffer.from(JSON.stringify({ type: "live_event", seq: event.seq, body: event.body })),
        ),
      );
  }
}

/** http.Server の `upgrade` に relay を付ける（`server.js` が呼ぶ）。 */
export function attachLiveViewUpgrade(server: Server, opts: LiveViewUpgradeOptions = {}): void {
  server.on("upgrade", (req: IncomingMessage, socket: Duplex, head: Buffer) => {
    handleLiveViewUpgrade(req, socket, head, opts).catch(() => rejectUpgrade(socket, 500, "internal_error"));
  });
}

/** テスト用: 開いている接続を全て破棄する。 */
export function resetLiveViewRelayConnectionsForTest(): void {
  closeAllLiveViewConnections(1001);
  connections.clear();
}
