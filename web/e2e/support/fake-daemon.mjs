import { readFileSync } from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);
const reservedPorts = new Set([7700, 7701, 7710, 7711, 7712]);
const delayValues = new Set([0, 5000, 10000]);

function resolve(node) {
  return node.$ref ? schema.$defs[node.$ref.split("/").at(-1)] : node;
}

export function fixtureFor(node, seen = new Set()) {
  node = resolve(node);
  if (Object.hasOwn(node, "const")) return node.const;
  if (node.enum) return node.enum[0];
  if (node.anyOf || node.oneOf) return fixtureFor((node.anyOf ?? node.oneOf)[0], seen);
  if (node.allOf) return Object.assign({}, ...node.allOf.map((part) => fixtureFor(part, seen)));
  const kind = Array.isArray(node.type) ? (node.type.find((item) => item !== "null") ?? "null") : node.type;
  if (kind === "object" || node.properties) {
    if (seen.has(node)) throw new Error("recursive required fixture schema");
    const next = new Set(seen).add(node);
    return Object.fromEntries((node.required ?? []).map((key) => [key, fixtureFor(node.properties[key], next)]));
  }
  if (kind === "array") return [];
  if (kind === "string") return node.format === "date-time" ? "2026-01-01T00:00:00Z" : "fixture";
  if (kind === "number" || kind === "integer") return Math.max(0, node.minimum ?? 0);
  if (kind === "boolean") return false;
  if (kind === "null") return null;
  throw new Error(`unsupported required fixture schema: ${JSON.stringify(node)}`);
}

export function validateFixture(value, original, location = "$", errors = []) {
  const node = resolve(original);
  if (node.anyOf || node.oneOf) {
    if (!(node.anyOf ?? node.oneOf).some((part) => validateFixture(value, part).length === 0))
      errors.push(`${location}: no matching variant`);
    return errors;
  }
  if (node.allOf) {
    for (const part of node.allOf) validateFixture(value, part, location, errors);
    return errors;
  }
  if (Object.hasOwn(node, "const") && value !== node.const) errors.push(`${location}: const`);
  if (node.enum && !node.enum.includes(value)) errors.push(`${location}: enum`);
  const kinds = Array.isArray(node.type) ? node.type : [node.type];
  const valid = kinds.some(
    (kind) =>
      (kind === "object" && value !== null && typeof value === "object" && !Array.isArray(value)) ||
      (kind === "array" && Array.isArray(value)) ||
      (kind === "integer" && Number.isInteger(value)) ||
      (kind === "number" && typeof value === "number") ||
      (kind === "string" && typeof value === "string") ||
      (kind === "boolean" && typeof value === "boolean") ||
      (kind === "null" && value === null),
  );
  if (node.type && !valid) {
    errors.push(`${location}: type`);
    return errors;
  }
  if (typeof value === "number" && node.minimum !== undefined && value < node.minimum)
    errors.push(`${location}: minimum`);
  if (typeof value === "string" && node.pattern && !new RegExp(node.pattern).test(value))
    errors.push(`${location}: pattern`);
  if (Array.isArray(value) && node.items)
    value.forEach((item, i) => {
      validateFixture(item, node.items, `${location}[${i}]`, errors);
    });
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const key of node.required ?? []) if (!Object.hasOwn(value, key)) errors.push(`${location}.${key}: required`);
    for (const [key, item] of Object.entries(value)) {
      if (node.properties?.[key]) validateFixture(item, node.properties[key], `${location}.${key}`, errors);
      else if (node.additionalProperties === false) errors.push(`${location}.${key}: unexpected`);
      else if (typeof node.additionalProperties === "object")
        validateFixture(item, node.additionalProperties, `${location}.${key}`, errors);
    }
  }
  return errors;
}

export const defaultFixtures = Object.fromEntries(
  ["health", "inbox", "daemon"].flatMap((key) => {
    const value = fixtureFor(schema.properties[key]);
    const errors = validateFixture(value, schema.properties[key]);
    if (errors.length) throw new Error(`${key}: ${errors.join(", ")}`);
    return [
      [`/api/v1/${key}`, value],
      [`/${key}`, value],
    ];
  }),
);

// files は daemon の path（`/api/v1/tasks/...`）→ `{ body, type?, disposition? }`。
// token を与えると、`Authorization: Bearer <token>` の無い要求に 401 を返す（P1-07 の中継の検査）。
export function createFakeDaemon({
  host = "127.0.0.1",
  port = 0,
  delayMs = 0,
  fixtures = defaultFixtures,
  token = null,
  files = {},
} = {}) {
  if (host !== "127.0.0.1" && host !== "::1" && host !== "localhost") throw new Error("fake daemon requires loopback");
  if (!Number.isInteger(port) || port < 0 || port > 65535 || reservedPorts.has(port))
    throw new Error("fake daemon refuses reserved port");
  if (!delayValues.has(delayMs)) throw new Error("JSON delay must be 0, 5000 or 10000 ms");
  const requests = [];
  const clients = new Set();
  let delay = delayMs;
  let timer;
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url ?? "/", `http://${host === "::1" ? "[::1]" : host}`).pathname;
    const record = {
      path: pathname,
      method: req.method,
      at: Date.now(),
      aborted: false,
      authorization: req.headers.authorization ?? null,
      cookie: req.headers.cookie ?? null,
    };
    requests.push(record);
    let finished = false;
    res.on("finish", () => {
      finished = true;
    });
    res.on("close", () => {
      if (!finished) record.aborted = true;
      clients.delete(res);
    });
    if (token !== null && req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "unauthorized" }));
      return;
    }
    if (pathname === "/events" || pathname === "/api/v1/events") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write(": connected\n\n");
      clients.add(res);
      return;
    }
    const file = files[pathname];
    if (file) {
      // run のファイル・成果物（P1-08）。単一の `bytes=a-b` の Range だけを扱う。
      const body = Buffer.from(file.body);
      const headers = { "content-type": file.type ?? "text/plain", "accept-ranges": "bytes" };
      if (file.disposition) headers["content-disposition"] = file.disposition;
      const range = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range ?? "");
      if (range) {
        const start = Number(range[1]);
        const end = Math.min(range[2] ? Number(range[2]) : body.length - 1, body.length - 1);
        if (start >= body.length || end < start) {
          res.writeHead(416, { "content-range": `bytes */${body.length}` });
          return res.end();
        }
        res.writeHead(206, { ...headers, "content-range": `bytes ${start}-${end}/${body.length}` });
        return res.end(body.subarray(start, end + 1));
      }
      res.writeHead(200, headers);
      return res.end(body);
    }
    const value = fixtures[pathname] ?? fixtures[pathname.replace(/^\/api\/v1/, "")];
    const respond = () => {
      if (res.destroyed) return;
      res.writeHead(value === undefined ? 404 : 200, { "content-type": "application/json" });
      res.end(JSON.stringify(value === undefined ? { error: "not found" } : value));
    };
    if (delay) setTimeout(respond, delay);
    else respond();
  });
  const sendEvent = (event, data = {}) => {
    const frame = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
    for (const client of clients) client.write(frame);
  };
  return {
    requests,
    sendEvent,
    setDelay(value) {
      if (!delayValues.has(value)) throw new Error("JSON delay must be 0, 5000 or 10000 ms");
      delay = value;
    },
    async start() {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(port, host, resolve);
      });
      timer = setInterval(() => sendEvent("daemon", fixtureFor(schema.$defs.DaemonSnapshot)), 2000);
      return `http://${host === "::1" ? "[::1]" : host}:${server.address().port}`;
    },
    async close() {
      clearInterval(timer);
      for (const client of clients) client.destroy();
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
    },
  };
}
