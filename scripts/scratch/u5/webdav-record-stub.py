#!/usr/bin/env python3
# U5: recording WebDAV stub (loopback only). Logs method, path, status, body length.
# usage: stub.py <port> <logfile> [ok|500]
import sys, http.server, time
store = {}
dirs = set([""])
LOG = open(sys.argv[2], "a", buffering=1)
MODE = sys.argv[3] if len(sys.argv) > 3 else "ok"


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def rec(self, code, n):
        LOG.write(
            f"{time.time():.3f} {self.command} {self.path} -> {code} len={n} "
            f"auth={self.headers.get('Authorization')} depth={self.headers.get('Depth')} "
            f"ctype={self.headers.get('Content-Type')}\n"
        )

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def send(self, code, data=b"", ctype="application/octet-stream"):
        self.send_response(code)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Content-Type", ctype)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def p(self):
        return self.path.split("?")[0].strip("/")

    def do_GET(self):
        self.body()
        if MODE == "slow" and "/.sccache_check" not in self.path:
            time.sleep(8)
        if MODE == "500":
            self.rec(500, 0)
            return self.send(500)
        d = store.get(self.p())
        if d is None:
            self.rec(404, 0)
            return self.send(404)
        self.rec(200, len(d))
        self.send(200, d)

    do_HEAD = do_GET

    def do_PUT(self):
        b = self.body()
        if MODE == "slow" and "/.sccache_check" not in self.path:
            time.sleep(8)
        if MODE == "500":
            self.rec(500, len(b))
            return self.send(500)
        store[self.p()] = b
        self.rec(201, len(b))
        self.send(201)

    def do_MKCOL(self):
        self.body()
        dirs.add(self.p())
        self.rec(201, 0)
        self.send(201)

    def do_PROPFIND(self):
        self.body()
        p = self.p()
        if self.path.endswith("/") or p in dirs or p in store:
            if self.path.endswith("/") or p in dirs:
                prop = "<D:resourcetype><D:collection/></D:resourcetype>"
            else:
                prop = "<D:resourcetype/><D:getcontentlength>%d</D:getcontentlength>" % len(store[p])
            x = (
                '<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response>'
                f"<D:href>{self.path}</D:href><D:propstat><D:prop>{prop}<D:getlastmodified>Mon, 28 Sep 2026 00:00:00 GMT</D:getlastmodified></D:prop>"
                "<D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"
            ).encode()
            self.rec(207, len(x))
            return self.send(207, x, "application/xml")
        self.rec(404, 0)
        self.send(404)

    def other(self):
        self.body()
        self.rec(405, 0)
        self.send(405)

    do_DELETE = other
    do_PROPPATCH = other
    do_MOVE = other
    do_COPY = other
    do_POST = other


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
