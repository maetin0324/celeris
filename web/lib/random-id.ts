/**
 * 乱数の UUID v4 を返す。`crypto.randomUUID` は secure context（https か localhost）でしか定義されないため、
 * LAN の http（例: http://192.168.1.103:7721）では TypeError になる。`crypto.getRandomValues` は
 * secure context でなくても使えるので、それで同じ形の UUID を作る。
 */
export function randomId(): string {
  const c = globalThis.crypto;
  if (typeof c?.randomUUID === "function" && globalThis.isSecureContext !== false) {
    return c.randomUUID();
  }
  const b = new Uint8Array(16);
  c.getRandomValues(b);
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
