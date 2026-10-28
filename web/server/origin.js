import { isIP } from "node:net";

// 自 origin の判定（HTTPS 前段の後ろで動かす。docs/ops/web-operation.md「HTTPS 前段（tailscale serve）」）。
// - 既定は接続そのもの: TLS なら https、平文なら http。Host はそのまま使う。
// - CELERIS_WEB_PUBLIC_ORIGINS（https の origin をカンマ区切り）に挙げた origin だけ、前段が TLS を終端して
//   平文で渡した要求を https として扱う。条件は 3 つ全部: 接続元が loopback、Host がその origin の host と一致、
//   X-Forwarded-Proto が無いか https。X-Forwarded-Proto だけでは https にしない。
// - 一致した要求は Origin 検査（CSRF・terminal・browser-live）の自 origin がその https origin になり、
//   session cookie に Secure が付く。一致しない要求の扱いは従来どおり。

function canonicalAuthority(authority) {
  if (typeof authority !== "string" || !authority || /[\s/@?#\\]/.test(authority)) return null;
  const match = /^(?:\[([0-9a-fA-F:]+)\]|([a-zA-Z0-9.-]+))(?::(\d{1,5}))?$/.exec(authority);
  if (!match) return null;
  if (match[1] && isIP(match[1]) !== 6) return null;
  const port = match[3] === undefined ? null : Number(match[3]);
  if (port !== null && (port < 1 || port > 65535)) return null;
  const host = (match[1] ? `[${match[1]}]` : match[2]).toLowerCase();
  return port === null || port === 443 ? host : `${host}:${port}`;
}

export function parsePublicOrigins(value = "") {
  const origins = [];
  for (const raw of value.split(",")) {
    const item = raw.trim();
    if (!item) continue;
    const match = /^https:\/\/([^/?#]+)\/?$/i.exec(item);
    const authority = match ? canonicalAuthority(match[1]) : null;
    if (!authority)
      throw new Error(`CELERIS_WEB_PUBLIC_ORIGINS has an invalid origin (https://host[:port] only): ${item}`);
    origins.push(`https://${authority}`);
  }
  return origins;
}

export function isLoopbackAddress(address) {
  if (typeof address !== "string") return false;
  const plain = address.toLowerCase().startsWith("::ffff:") ? address.slice(7) : address;
  const kind = isIP(plain);
  return (kind === 4 && plain.startsWith("127.")) || (kind === 6 && plain === "::1");
}

export function createOriginPolicy({ publicOrigins = [] } = {}) {
  const origins = new Set(publicOrigins);

  // 前段経由と認めた要求なら公開 origin（https://…）、そうでなければ null。
  function forwarded(req) {
    if (origins.size === 0 || req.socket?.encrypted) return null;
    if (!isLoopbackAddress(req.socket?.remoteAddress)) return null;
    const proto = req.headers["x-forwarded-proto"];
    if (proto !== undefined && (typeof proto !== "string" || proto.split(",")[0].trim().toLowerCase() !== "https"))
      return null;
    const authority = canonicalAuthority(req.headers.host);
    const candidate = authority ? `https://${authority}` : null;
    return candidate && origins.has(candidate) ? candidate : null;
  }

  return {
    publicOrigins: [...origins],
    // Allowed Host に足す host 名（port なし、IPv6 は括弧なし）。
    hosts: () => [...origins].map((origin) => new URL(origin).hostname.replace(/^\[|\]$/g, "")),
    selfOrigin: (req) => forwarded(req) ?? `${req.socket?.encrypted ? "https" : "http"}://${req.headers.host}`,
    secure: (req) => Boolean(req.socket?.encrypted) || forwarded(req) !== null,
  };
}
