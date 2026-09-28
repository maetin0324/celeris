#!/usr/bin/env bash
# Phase SD-1: verify.sh が
#   (1) 検査ごとの所要秒数を verify.json の `checks[].secs` に、準備・ロック待ち・全体を `durations` に書く、
#   (2) 検査 4b（gui-e2e）と検査 5（N-1）を並行に回す（壁時計が両者の和より短い）、
#   (3) checks の順は 1, 2, 3, 4, 4b, 5, 6 のまま、
# であることを確かめる。偽の celeris（python3 の HTTP サーバ）・偽の GUI（node の HTTP サーバ）・偽の pnpm を
# **空いているポート**（SD_STAGING_*_PORT）で起こす。本番の DB・ポート・ネットワークには触れない。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
# `SD_TEST_KEEP=<dir>` なら verify.json と verify.sh の出力をそこに写してから消す（調べる用）。
trap '[ -n "${SD_TEST_KEEP:-}" ] && cp "$root"/state/releases/aaaaaaaaaaaa/verify.json "$root"/verify.out "$SD_TEST_KEEP"/ 2>/dev/null; rm -rf "$root"' EXIT
command -v sqlite3 >/dev/null 2>&1 || { echo "verify_durations_and_parallel: skipped (no sqlite3)"; exit 0; }
[ -x /usr/bin/node ] || { echo "verify_durations_and_parallel: skipped (no /usr/bin/node)"; exit 0; }

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/verify.out" ] && tail -n 40 "$root/verify.out" >&2
  exit 1
}

free_ports() {
  python3 - <<'PY'
import socket
socks = []
for _ in range(3):
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    socks.append(s)
print(" ".join(str(s.getsockname()[1]) for s in socks))
PY
}
read -r API_PORT GUI_PORT OLD_PORT <<<"$(free_ports)"

mkdir -p "$root/bin" "$root/config" "$root/state/releases"
: >"$root/config/config.toml"

# ---- 本番 DB の代わり（空の表と schema_migrations = 5） --------------------------
sqlite3 "$root/prod.sqlite3" <<'SQL'
CREATE TABLE tasks (id TEXT, status TEXT);
CREATE TABLE projects (id TEXT);
CREATE TABLE milestones (id TEXT);
CREATE TABLE org_nodes (id TEXT);
CREATE TABLE approvals (id TEXT, decision TEXT);
CREATE TABLE reports (id TEXT, node_id TEXT);
CREATE TABLE messages (id TEXT, node_id TEXT);
CREATE TABLE schema_migrations (version INTEGER);
INSERT INTO schema_migrations VALUES (5);
SQL

# ---- 偽の celeris（`FAKE_START_DELAY` 秒待ってから listen する） ------------------
cat >"$root/fake-celeris.py" <<'PY'
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

args = sys.argv[1:]
listen = args[args.index("--listen") + 1]
release = args[args.index("--release") + 1]
host, port = listen.rsplit(":", 1)
time.sleep(float(os.environ.get("FAKE_START_DELAY", "0")))


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        p = self.path.split("?")[0]
        if p == "/api/v1/health":
            return self.send({"schema_version": 5, "mode": "verify", "release": release})
        if p == "/api/v1/tasks":
            return self.send({"items": [], "next_cursor": None, "total": 0})
        if p == "/api/v1/tasks/t1":
            return self.send({"task": {"status": "done"}})
        if p == "/api/v1/tasks/t1/events":
            return self.send({"items": [{"event": {"type": "worker_started"}},
                                        {"event": {"type": "worker_finished", "outcome": "done: fake"}}]})
        if p == "/api/v1/reports":
            return self.send({"items": [{"id": "r1", "task_id": "t1"}]})
        if p in ("/api/v1/projects", "/api/v1/org", "/api/v1/approvals"):
            return self.send({"items": []})
        return self.send({})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        return self.send({"id": "t1", "status": "ready"})


ThreadingHTTPServer((host, int(port)), H).serve_forever()
PY

new12=aaaaaaaaaaaa
old12=bbbbbbbbbbbb
for sha in "$new12" "$old12"; do
  rel="$root/state/releases/$sha"
  mkdir -p "$rel/bin" "$rel/gui/node_modules/@playwright/test"
  printf '{"sha":"%s","sha12":"%s","schema_version":5}\n' "$sha" "$sha" >"$rel/manifest.json"
  printf '{"ok":true}\n' >"$rel/gate.json"
  cat >"$rel/gui/server.js" <<'JS'
const http = require("http");
const [host, port] = process.env.CELERIS_GUI_BIND.split(":");
http.createServer((req, res) => {
  if (req.url === "/healthz") {
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ release: process.env.CELERIS_GUI_RELEASE }));
    return;
  }
  res.end("ok");
}).listen(Number(port), host);
JS
done
# 新しい版はすぐ起き、N-1 は 3 秒遅れて起きる（検査 5 に時間がかかる状況）。
printf '#!/usr/bin/env bash\nexec python3 %q "$@"\n' "$root/fake-celeris.py" >"$root/state/releases/$new12/bin/celeris"
printf '#!/usr/bin/env bash\nFAKE_START_DELAY=3 exec python3 %q "$@"\n' "$root/fake-celeris.py" >"$root/state/releases/$old12/bin/celeris"
chmod +x "$root/state/releases/$new12/bin/celeris" "$root/state/releases/$old12/bin/celeris"
ln -s "releases/$old12" "$root/state/current"

# 偽の pnpm: `e2e:staging` は 3 秒かかって成功する。
cat >"$root/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = e2e:staging ] && sleep 3
exit 0
EOF
chmod +x "$root/bin/pnpm"

CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
  CELERIS_DB="$root/prod.sqlite3" SD_REPO="$root/no-repo" SD_PNPM_SHIM_DIR="$root/bin" \
  SD_STAGING_API_PORT="$API_PORT" SD_STAGING_GUI_PORT="$GUI_PORT" SD_STAGING_OLD_API_PORT="$OLD_PORT" \
  PATH="$root/bin:$PATH" bash "$here/verify.sh" "$new12" >"$root/verify.out" 2>&1 \
  || fail "verify.sh failed"

python3 - "$root/state/releases/$new12/verify.json" <<'PY' || fail "verify.json does not have the expected durations"
import json, sys
v = json.load(open(sys.argv[1], encoding="utf-8"))
assert v["ok"] is True and v["live_ok"] is True, v
ids = [c["id"] for c in v["checks"]]
assert ids == ["1", "2", "3", "4", "4b", "5", "6"], ids
for c in v["checks"]:
    assert isinstance(c["secs"], (int, float)) and c["secs"] >= 0, c
secs = {c["id"]: c["secs"] for c in v["checks"]}
assert secs["4b"] >= 2.5, secs   # 偽の e2e は 3 秒
assert secs["5"] >= 2.5, secs    # N-1 は 3 秒遅れて起きる
d = v["durations"]
for k in ("lock_wait_s", "prepare_s", "parallel_4b_5_s", "total_s"):
    assert isinstance(d[k], (int, float)), (k, d)
# 並行: 4b と 5 の壁時計は和より十分短い（直列なら 6 秒以上）。
assert d["parallel_4b_5_s"] < secs["4b"] + secs["5"] - 1.5, (d, secs)
assert d["total_s"] >= d["parallel_4b_5_s"], d
PY

# 起こしたプロセスは全部止まっている（後始末）。
for port in "$API_PORT" "$GUI_PORT" "$OLD_PORT"; do
  if ss -ltn "sport = :$port" 2>/dev/null | tail -n +2 | grep -q .; then fail "port $port is still listening"; fi
done
echo "verify_durations_and_parallel: ok"
