import { createHash, randomBytes } from "node:crypto";
import { connect, type Socket } from "node:net";

/**
 * 最小の RFC 6455 実装（ADR-0080 D6 の Live View relay 用。新しい依存を足さない）。
 * - 拡張（permessage-deflate 等）・subprotocol は交渉しない
 * - 分割（continuation）は組み立てる。制御 frame は分割中にも受ける
 * - 受け手の向きで mask の有無を必須にする（client→server は mask 必須、server→client は mask 禁止）
 */

export const WS_OP = { CONT: 0x0, TEXT: 0x1, BINARY: 0x2, CLOSE: 0x8, PING: 0x9, PONG: 0xa } as const;

const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

export class WsProtocolError extends Error {
  readonly closeCode: number;
  constructor(message: string, closeCode = 1002) {
    super(message);
    this.closeCode = closeCode;
  }
}

export interface WsMessage {
  opcode: number;
  payload: Buffer;
}

/** `Sec-WebSocket-Accept` の値。 */
export function wsAcceptKey(key: string): string {
  return createHash("sha1").update(`${key}${WS_GUID}`, "latin1").digest("base64");
}

/** `Sec-WebSocket-Key` の形（base64 の 16 byte）か。 */
export function validWsKey(key: unknown): key is string {
  return typeof key === "string" && /^[A-Za-z0-9+/]{22}==$/.test(key);
}

/** 1 frame（fin=1）を作る。`mask` は client→server の向きで true。 */
export function encodeWsFrame(opcode: number, payload: Buffer = Buffer.alloc(0), mask = false): Buffer {
  const len = payload.length;
  let headerLen = 2;
  if (len >= 126 && len <= 0xffff) headerLen += 2;
  else if (len > 0xffff) headerLen += 8;
  if (mask) headerLen += 4;
  const out = Buffer.alloc(headerLen + len);
  out[0] = 0x80 | (opcode & 0x0f);
  let off = 2;
  if (len < 126) {
    out[1] = len;
  } else if (len <= 0xffff) {
    out[1] = 126;
    out.writeUInt16BE(len, 2);
    off = 4;
  } else {
    out[1] = 127;
    out.writeBigUInt64BE(BigInt(len), 2);
    off = 10;
  }
  if (mask) {
    out[1] = (out[1] ?? 0) | 0x80;
    const key = randomBytes(4);
    key.copy(out, off);
    off += 4;
    for (let i = 0; i < len; i++) out[off + i] = (payload[i] ?? 0) ^ (key[i & 3] ?? 0);
  } else {
    payload.copy(out, off);
  }
  return out;
}

/** close frame の payload（code + 短い理由）。 */
export function wsClosePayload(code: number, reason = ""): Buffer {
  const r = Buffer.from(reason.slice(0, 100), "utf8");
  const out = Buffer.alloc(2 + r.length);
  out.writeUInt16BE(code, 0);
  r.copy(out, 2);
  return out;
}

/** close frame の payload から code を読む（無ければ 1005）。 */
export function wsCloseCode(payload: Buffer): number {
  return payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
}

/**
 * 受信した byte 列を message に組み立てる。
 * `expectMasked`: server 側（client からの frame）は true、client 側（server からの frame）は false。
 */
export class WsDecoder {
  readonly #expectMasked: boolean;
  readonly #maxMessageBytes: number;
  #buf: Buffer = Buffer.alloc(0);
  #fragOpcode: number | null = null;
  #fragParts: Buffer[] = [];
  #fragLen = 0;

  constructor(opts: { expectMasked: boolean; maxMessageBytes: number }) {
    this.#expectMasked = opts.expectMasked;
    this.#maxMessageBytes = opts.maxMessageBytes;
  }

  /** 読めた分の message を返す。規約違反は WsProtocolError。 */
  push(chunk: Buffer): WsMessage[] {
    this.#buf = this.#buf.length === 0 ? chunk : Buffer.concat([this.#buf, chunk]);
    const out: WsMessage[] = [];
    for (;;) {
      const buf = this.#buf;
      if (buf.length < 2) break;
      const b0 = buf[0] ?? 0;
      const b1 = buf[1] ?? 0;
      const fin = (b0 & 0x80) !== 0;
      if ((b0 & 0x70) !== 0) throw new WsProtocolError("reserved bits set");
      const opcode = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      if (masked !== this.#expectMasked) throw new WsProtocolError("unexpected mask bit");
      let len = b1 & 0x7f;
      let off = 2;
      if (len === 126) {
        if (buf.length < 4) break;
        len = buf.readUInt16BE(2);
        off = 4;
      } else if (len === 127) {
        if (buf.length < 10) break;
        const big = buf.readBigUInt64BE(2);
        if (big > BigInt(this.#maxMessageBytes)) throw new WsProtocolError("frame too large", 1009);
        len = Number(big);
        off = 10;
      }
      const isControl = (opcode & 0x08) !== 0;
      if (isControl && (!fin || len > 125)) throw new WsProtocolError("bad control frame");
      if (len > this.#maxMessageBytes) throw new WsProtocolError("frame too large", 1009);
      const maskLen = masked ? 4 : 0;
      if (buf.length < off + maskLen + len) break;
      let payload: Buffer;
      if (masked) {
        const key = buf.subarray(off, off + 4);
        payload = Buffer.alloc(len);
        const start = off + 4;
        for (let i = 0; i < len; i++) payload[i] = (buf[start + i] ?? 0) ^ (key[i & 3] ?? 0);
      } else {
        payload = Buffer.from(buf.subarray(off, off + len));
      }
      this.#buf = buf.subarray(off + maskLen + len);
      if (isControl) {
        if (opcode !== WS_OP.CLOSE && opcode !== WS_OP.PING && opcode !== WS_OP.PONG) {
          throw new WsProtocolError("unknown control opcode");
        }
        out.push({ opcode, payload });
        continue;
      }
      if (opcode === WS_OP.CONT) {
        if (this.#fragOpcode === null) throw new WsProtocolError("unexpected continuation");
      } else if (opcode === WS_OP.TEXT || opcode === WS_OP.BINARY) {
        if (this.#fragOpcode !== null) throw new WsProtocolError("interleaved data frame");
        this.#fragOpcode = opcode;
      } else {
        throw new WsProtocolError("unknown data opcode");
      }
      this.#fragLen += payload.length;
      if (this.#fragLen > this.#maxMessageBytes) throw new WsProtocolError("message too large", 1009);
      this.#fragParts.push(payload);
      if (fin) {
        const parts = this.#fragParts;
        out.push({
          opcode: this.#fragOpcode ?? WS_OP.BINARY,
          payload: parts.length === 1 ? (parts[0] ?? Buffer.alloc(0)) : Buffer.concat(parts),
        });
        this.#fragOpcode = null;
        this.#fragParts = [];
        this.#fragLen = 0;
      }
    }
    return out;
  }
}

/** HTTP の状態行＋ヘッダ（`\r\n\r\n` まで）を読む。 */
function parseHttpHead(head: string): { status: number; headers: Map<string, string> } | null {
  const lines = head.split("\r\n");
  const m = /^HTTP\/1\.1 (\d{3})(?: |$)/.exec(lines[0] ?? "");
  if (!m) return null;
  const headers = new Map<string, string>();
  for (const line of lines.slice(1)) {
    const c = line.indexOf(":");
    if (c <= 0) continue;
    headers.set(line.slice(0, c).trim().toLowerCase(), line.slice(c + 1).trim());
  }
  return { status: Number(m[1]), headers };
}

export interface WsClientConnection {
  socket: Socket;
  /** 101 応答の後ろに続いて届いていた byte 列（最初の frame の一部）。 */
  leftover: Buffer;
}

/**
 * client 側の handshake を行う（relay の upstream 側と、テストの client）。
 * `headers` は `Host` 以外の追加ヘッダ（`Origin` / `Cookie` 等）。成功しなければ例外（理由は固定の短い文字列）。
 */
export function openWsClient(opts: {
  host: string;
  port: number;
  path: string;
  hostHeader: string;
  headers?: Record<string, string>;
  timeoutMs?: number;
}): Promise<WsClientConnection> {
  return new Promise((resolve, reject) => {
    const key = randomBytes(16).toString("base64");
    const socket = connect({ host: opts.host, port: opts.port });
    let buf = Buffer.alloc(0);
    let done = false;
    const fail = (reason: string) => {
      if (done) return;
      done = true;
      socket.destroy();
      reject(new Error(reason));
    };
    socket.setTimeout(opts.timeoutMs ?? 10_000, () => fail("ws_handshake_timeout"));
    socket.once("error", () => fail("ws_connect_failed"));
    socket.once("close", () => fail("ws_closed_during_handshake"));
    socket.on("connect", () => {
      const lines = [
        `GET ${opts.path} HTTP/1.1`,
        `Host: ${opts.hostHeader}`,
        "Upgrade: websocket",
        "Connection: Upgrade",
        `Sec-WebSocket-Key: ${key}`,
        "Sec-WebSocket-Version: 13",
      ];
      for (const [name, value] of Object.entries(opts.headers ?? {})) lines.push(`${name}: ${value}`);
      socket.write(`${lines.join("\r\n")}\r\n\r\n`);
    });
    const onData = (chunk: Buffer) => {
      buf = Buffer.concat([buf, chunk]);
      const end = buf.indexOf("\r\n\r\n");
      if (end < 0) {
        if (buf.length > 16 * 1024) fail("ws_bad_handshake");
        return;
      }
      const head = parseHttpHead(buf.subarray(0, end).toString("latin1"));
      const leftover = Buffer.from(buf.subarray(end + 4));
      if (head?.status !== 101) {
        const status = head?.status ?? 0;
        fail(`ws_http_${status}`);
        return;
      }
      if (
        head.headers.get("upgrade")?.toLowerCase() !== "websocket" ||
        head.headers.get("sec-websocket-accept") !== wsAcceptKey(key) ||
        head.headers.has("sec-websocket-extensions")
      ) {
        fail("ws_bad_handshake");
        return;
      }
      done = true;
      socket.off("data", onData);
      socket.removeAllListeners("close");
      socket.removeAllListeners("error");
      socket.removeAllListeners("timeout");
      socket.setTimeout(0);
      resolve({ socket, leftover });
    };
    socket.on("data", onData);
  });
}
