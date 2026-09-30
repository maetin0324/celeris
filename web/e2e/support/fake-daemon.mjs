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
  ["health", "inbox", "daemon", "console"].flatMap((key) => {
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
  const consoleClients = new Set();
  let delay = delayMs;
  let postDelay = 0;
  let streamStatus = 200;
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
    if (pathname === "/api/v1/stream" && streamStatus !== 200) {
      res.writeHead(streamStatus, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "too_many_streams" }));
      return;
    }
    // ops: daemon/providers (P4-12)
    if (pathname === "/api/v1/replay" || pathname.startsWith("/api/v1/providers") || pathname === "/api/v1/reload") {
      if (!server.opsProviders)
        server.opsProviders = {
          items: [
            {
              id: "claude-main",
              adapter: "claude-code",
              concurrency: 2,
              env_keys: [],
              tiers: ["standard"],
              model: "sonnet",
              stats: {
                by_day: [],
                done: 0,
                error: 0,
                input_tokens: 0,
                lease_expired: 0,
                output_tokens: 0,
                question: 0,
                requeue: 0,
                runs: 0,
              },
            },
          ],
        };
      const ops = server.opsProviders;
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        record.body = text;
        const json = (status, body) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(body));
        };
        let input = {};
        try {
          input = text ? JSON.parse(text) : {};
        } catch {
          return json(400, { error: "bad json" });
        }
        const parts = pathname.replace("/api/v1/", "").split("/");
        if (pathname === "/api/v1/replay" && req.method === "POST") return json(200, { mismatches: [], tasks: 3 });
        if (pathname === "/api/v1/reload" && req.method === "POST") return json(200, { reloaded: true });
        if (parts[0] !== "providers") return json(404, { error: "not found" });
        const found = ops.items.find((item) => item.id === parts[1]);
        if (parts.length === 1 && req.method === "GET") return json(200, { items: ops.items });
        if (parts.length === 1 && req.method === "POST") {
          if (!input.id || ops.items.some((item) => item.id === input.id))
            return json(422, { detail: "id が不正か重複しています" });
          const created = { concurrency: 1, env_keys: [], tiers: [], stats: ops.items[0]?.stats, ...input };
          ops.items.push(created);
          return json(200, created);
        }
        if (!found) return json(404, { error: "not found" });
        if (parts.length === 2 && req.method === "PATCH") return json(200, Object.assign(found, input));
        if (parts.length === 2 && req.method === "DELETE") {
          ops.items.splice(ops.items.indexOf(found), 1);
          return json(200, {});
        }
        if (parts[2] === "check" && req.method === "POST") {
          found.last_check = { at: "2026-09-30T00:00:00Z", result: "ok", detail: null };
          return json(200, { checked_at: "2026-09-30T00:00:00Z", result: "ok", detail: null });
        }
        return json(404, { error: "not found" });
      });
      return;
    }
    // end ops: daemon/providers (P4-12)
    if (pathname === "/events" || pathname === "/api/v1/events" || pathname === "/api/v1/stream") {
      record.query = new URL(req.url ?? "/", "http://x").search;
      record.lastEventId = req.headers["last-event-id"] ?? null;
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write(": connected\n\n");
      clients.add(res);
      return;
    }
    if (pathname === "/api/v1/console/stream") {
      record.query = new URL(req.url ?? "/", "http://x").search;
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      const since = new URL(req.url ?? "/", "http://x").searchParams.get("since");
      res.write(
        `event: hello\ndata: ${JSON.stringify({ cursor: since ?? "h0", now: "2026-01-01T00:00:00Z", scope: "all" })}\n\n`,
      );
      consoleClients.add(res);
      res.on("close", () => consoleClients.delete(res));
      return;
    }
    if (
      req.method === "POST" &&
      (pathname === "/api/v1/console/instruct" || pathname === "/api/v1/console/new-conversation")
    ) {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        record.body = Buffer.concat(chunks).toString("utf8");
        const respond = () => {
          if (pathname.endsWith("/instruct")) {
            res.writeHead(202, { "content-type": "application/json" });
            res.end(JSON.stringify({ message_id: "M1", node_id: "cos", task_id: "T1" }));
          } else {
            res.writeHead(204);
            res.end();
          }
        };
        if (postDelay) setTimeout(respond, postDelay);
        else respond();
      });
      return;
    }
    // ops: releases (P4-16)
    // GET /api/v1/releases は状態を持つ。POST /api/v1/releases/{sha12}/promote は 202 で昇格を起こし、
    // `pendingMs` の間は promoting、その後 `mode`（succeed | fail）の結果を行に書く。
    // 制御: POST /__fake/releases `{ mode?, pendingMs? }`。
    if (pathname === "/__fake/releases" || pathname.startsWith("/api/v1/releases")) {
      if (!server.fakeReleases) {
        server.fakeReleases = {
          mode: "succeed",
          pendingMs: 0,
          current: "aaaaaaaaaaaa",
          promotedAt: { aaaaaaaaaaaa: "2026-09-29T00:00:00Z", bbbbbbbbbbbb: null },
          promoting: null,
          failed: null,
        };
      }
      const rel = server.fakeReleases;
      const json = (status, value) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(value));
      };
      if (req.method === "POST") {
        const chunks = [];
        req.on("data", (chunk) => chunks.push(chunk));
        req.on("end", () => {
          const promote = /^\/api\/v1\/releases\/([0-9a-f]{12})\/promote$/.exec(pathname);
          if (pathname === "/__fake/releases") {
            const body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
            if (body.mode) rel.mode = body.mode;
            if (typeof body.pendingMs === "number") rel.pendingMs = body.pendingMs;
            return json(200, { mode: rel.mode, pendingMs: rel.pendingMs });
          }
          if (!promote || !(promote[1] in rel.promotedAt)) return json(404, { error: "not found" });
          const sha = promote[1];
          if (rel.promoting) return json(409, { error: "別の昇格が走っています" });
          const startedAt = new Date().toISOString();
          rel.promoting = sha;
          rel.failed = null;
          setTimeout(() => {
            if (rel.mode === "fail")
              rel.failed = { sha, failed_at: new Date().toISOString(), error: "promote.sh が exit 1 で終わりました" };
            else {
              rel.current = sha;
              rel.promotedAt[sha] = new Date().toISOString();
            }
            rel.promoting = null;
          }, rel.pendingMs);
          return json(202, { sha12: sha, log: `${sha}/promote.log`, started_at: startedAt, script_from: "current" });
        });
        return;
      }
      const item = (sha) => ({
        sha12: sha,
        gate_ok: true,
        is_current: rel.current === sha,
        is_previous: rel.current !== sha && rel.promotedAt[sha] !== null,
        promoting: rel.promoting === sha,
        promoted_at: rel.promotedAt[sha],
        promote_failed:
          rel.failed && rel.failed.sha === sha ? { failed_at: rel.failed.failed_at, error: rel.failed.error } : null,
        built_at: "2026-09-28T00:00:00Z",
        ref: "main",
      });
      return json(200, {
        current: rel.current,
        previous: null,
        running: { release: rel.current, role: "active", instance_id: "I1" },
        instances: [],
        items: ["bbbbbbbbbbbb", "aaaaaaaaaaaa"].map(item),
      });
    }
    // accounts/clusters (P4-13..15)
    // accounts・secrets・llm sources・mcp clients・clusters を状態付きで返す。ログイン・接続の途中状態（login_pending /
    // connect_pending）はサーバが持ち、GET で返す。secret の値は保存せず fingerprint だけ持つ。
    if (/^\/api\/v1\/(accounts|secrets|llm\/sources|mcp\/clients|clusters)(\/|$)/.test(pathname)) {
      if (!server.fakeAc) {
        server.fakeAc = {
          accounts: [{ id: "main", adapter: "claude-code", logged_in: true, login_pending: false }],
          secrets: [],
          clusters: [
            {
              id: "pegasus",
              host: "pegasus.example",
              auth: "totp",
              connected: false,
              connect_pending: false,
              work_dir: null,
            },
          ],
        };
      }
      const ac = server.fakeAc;
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        const json = (status, value) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(value));
        };
        let input = {};
        try {
          input = text ? JSON.parse(text) : {};
        } catch {
          return json(400, { error: "bad json" });
        }
        const parts = pathname.replace("/api/v1/", "").split("/");
        const view = (a) => ({
          dir: `/accounts/${a.id}`,
          in_use: 0,
          stats: {
            done: 0,
            error: 0,
            input_tokens: 0,
            lease_expired: 0,
            output_tokens: 0,
            question: 0,
            requeue: 0,
            runs: 0,
            by_day: [],
          },
          ...a,
        });
        if (parts[0] === "accounts") {
          const acc = ac.accounts.find((a) => a.id === parts[1]);
          if (parts.length === 1 && req.method === "GET")
            return json(200, { items: ac.accounts.map(view), max_runs_per_account: 1 });
          if (parts.length === 1 && req.method === "POST") {
            if (!input.id || ac.accounts.some((a) => a.id === input.id))
              return json(422, { detail: "id が不正か重複しています" });
            const created = {
              id: input.id,
              adapter: input.adapter ?? "claude-code",
              logged_in: false,
              login_pending: false,
            };
            ac.accounts.push(created);
            return json(200, view(created));
          }
          if (!acc) return json(404, { error: "not found" });
          if (parts.length === 2 && req.method === "DELETE") {
            ac.accounts.splice(ac.accounts.indexOf(acc), 1);
            return json(200, {});
          }
          if (parts[2] === "check" && req.method === "POST")
            return json(200, { checked_at: "2026-09-30T00:00:00Z", result: "ok" });
          if (parts[2] === "login" && parts.length === 3 && req.method === "POST") {
            acc.login_pending = true;
            return json(200, {
              kind: "paste_code",
              url: "https://login.example/auth",
              expires_at: "2026-09-30T01:00:00Z",
              user_code: "ABCD-1234",
            });
          }
          if (parts[2] === "login" && parts.length === 3 && req.method === "DELETE") {
            acc.login_pending = false;
            return json(200, {});
          }
          if (parts[2] === "login" && parts[3] === "code" && req.method === "POST") {
            if (!input.code) return json(422, { detail: "code が空です" });
            acc.login_pending = false;
            acc.logged_in = true;
            return json(200, { result: "ok" });
          }
        }
        if (parts[0] === "secrets") {
          if (parts.length === 1 && req.method === "GET") return json(200, { items: ac.secrets });
          if (parts.length === 2 && req.method === "PUT") {
            if (!input.value) return json(422, { detail: "値が空です" });
            ac.secrets = ac.secrets.filter((s) => s.id !== parts[1]);
            const secret = {
              id: parts[1],
              fingerprint: `fp-${input.value.length}`,
              updated_at: "2026-09-30T00:00:00Z",
              used_by: [],
            };
            ac.secrets.push(secret);
            return json(200, { id: secret.id, fingerprint: secret.fingerprint, updated_at: secret.updated_at });
          }
          if (parts.length === 2 && req.method === "DELETE") {
            ac.secrets = ac.secrets.filter((s) => s.id !== parts[1]);
            return json(200, {});
          }
        }
        if (parts[0] === "llm" && req.method === "GET")
          return json(200, {
            sources: [
              {
                id: "claude-oauth",
                kind: "oauth",
                enabled: true,
                last_hour_completion_tokens: 0,
                last_hour_prompt_tokens: 0,
                last_hour_requests: 3,
                accounts: [{ id: "main", logged_in: true }],
              },
            ],
          });
        if (parts[0] === "mcp" && parts.length === 2 && req.method === "GET")
          return json(200, {
            items: [{ id: "c1", name: "editor", created_at: "2026-09-01T00:00:00Z", scopes: ["tasks:read"] }],
          });
        if (parts[0] === "mcp" && parts[3] === "calls" && req.method === "GET")
          return json(200, {
            items: [
              {
                id: "k1",
                client_id: parts[2],
                at: "2026-09-30T00:00:00Z",
                tool: "tasks_list",
                ok: true,
                latency_ms: 12,
              },
            ],
          });
        if (parts[0] === "clusters") {
          const cl = ac.clusters.find((c) => c.id === parts[1]);
          const row = (c) => ({
            concurrency: 1,
            delete_on_push: false,
            env_keys: [],
            has_setup: false,
            rsync_excludes: [],
            sync: "rsync",
            ...c,
          });
          if (parts.length === 1 && req.method === "GET") return json(200, { items: ac.clusters.map(row) });
          if (!cl) return json(404, { error: "not found" });
          if (parts[2] === "connect" && parts.length === 3 && req.method === "POST") {
            cl.connect_pending = true;
            return json(200, { kind: "needs_code", prompt: "Verification code:", expires_at: "2026-09-30T01:00:00Z" });
          }
          if (parts[2] === "connect" && parts.length === 3 && req.method === "DELETE") {
            cl.connect_pending = false;
            return json(200, {});
          }
          if (parts[2] === "connect" && parts[3] === "code" && req.method === "POST") {
            if (!input.code) return json(422, { detail: "code が空です" });
            cl.connect_pending = false;
            cl.connected = true;
            return json(200, { ok: true });
          }
          if (parts[2] === "settings" && req.method === "PUT") {
            cl.work_dir = input.work_dir ?? null;
            cl.work_dir_source = input.work_dir ? "db" : null;
            return json(200, { cluster_id: cl.id, updated_at: "2026-09-30T00:00:00Z", work_dir: cl.work_dir });
          }
        }
        return json(404, { error: "not found" });
      });
      return;
    }
    const file = files[pathname];
    if (file) {
      // run のファイル・成果物（P1-08）。単一の `bytes=a-b` の Range だけを扱う。
      // body は関数でもよい（実行中の run の追記を試す。P3-12）。
      const body = Buffer.from(typeof file.body === "function" ? file.body() : file.body);
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
      // `offset`（P3-12 の追い掛け）: offset == size は空本体、offset > size は 416。
      const offsetParam = new URL(req.url ?? "/", "http://x").searchParams.get("offset");
      if (offsetParam !== null) {
        const offset = Number(offsetParam);
        if (offset > body.length) {
          res.writeHead(416, { "content-range": `bytes */${body.length}` });
          return res.end();
        }
        res.writeHead(200, headers);
        return res.end(body.subarray(offset));
      }
      res.writeHead(200, headers);
      return res.end(body);
    }
    const raw = fixtures[pathname] ?? fixtures[pathname.replace(/^\/api\/v1/, "")];
    const value = typeof raw === "function" ? raw(new URL(req.url ?? "/", "http://x")) : raw;
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
    // `/api/v1/stream` の応答を 200 以外（503 など）にする（P1-09）。
    setStreamStatus(value) {
      streamStatus = value;
    },
    // Console（P3-01）: `/api/v1/console/stream` の接続へ block を流す・切る。POST の応答を遅らせる。
    sendConsoleBlock(block) {
      const frame = `event: console.block\ndata: ${JSON.stringify(block)}\n\n`;
      for (const client of consoleClients) client.write(frame);
    },
    dropConsoleClients() {
      for (const client of consoleClients) client.destroy();
    },
    setPostDelay(value) {
      postDelay = value;
    },
    get consoleClients() {
      return consoleClients.size;
    },
    get streamClients() {
      return clients.size;
    },
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
      for (const client of [...clients, ...consoleClients]) client.destroy();
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
    },
  };
}
