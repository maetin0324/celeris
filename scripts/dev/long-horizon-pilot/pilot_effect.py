#!/usr/bin/env python3
"""pilot の公開相当の effect（実際の外部 publish はしない）。ADR 2026-11-05-phase3-p0d。

worker（実 LLM・stub）が使うのは client だけで、記録には触れない:

    pilot-effect publish <logical_id>          # driver が試行ごとに置く wrapper（socket 名を埋めてある）
    pilot_effect.py publish <logical_id> --socket <name>

記録を書くのは broker（driver が worker の外で起動する process）だけ:

    pilot_effect.py serve --dir <record dir> --socket <name> --api <daemon base> --token-file <f> --project <id>
        （HMAC の鍵は stdin から 1 行で受け、disk に書かない）

- 承認は本物の daemon API で読む: `GET /api/v1/approvals?project=<id>` の question に `pilot-effect:<logical_id>` を
  含む行のうち、最後に決まったもの。`once`・`standing` は承認、`denied` は却下。決定は、その task の
  `Answered` event の主体が `human:` で始まるときだけ数える（CoS・operator の決定は数えない）。
- publish の結果（exit）: 0 = 公開した（同じ logical_id の 2 回目以降も 0 で、記録は増えない）、3 = 承認が無い
  （未決・取り下げを含む。effect 0）、4 = 却下（effect 0）、5 = broker に届かない・daemon が読めない、2 = 使い方の誤り。
- 記録は `effects.jsonl`（効果）と `audit.jsonl`（すべての要求と判定）。各行は前の行の MAC を含む HMAC-SHA256 の鎖で、
  鍵は broker と driver の memory にしか無い。audit の `published` の行は効果の行の MAC を持ち、broker は追記の
  たびに鎖の末尾（seq と MAC）を stdout の pipe で driver に知らせる（`head` の行）。worker が行を足す・消す・
  書き換える・末尾を切ると `verify` が落ち、driver は facts を作らず observation_failed にする。file は 0444・
  dir は 0555（broker が書く間だけ開ける）。
- 同じ logical_id の効果は 1 件以下: 判定と追記は 1 つの lock（thread の lock と record の flock）の中で行う。
  追記は 1 回の write と fsync。broker が追記の途中で落ちて末尾が欠けた行は、次の起動で切り詰めて audit に残す。
- 試験用の hook（環境変数。試験だけが使う）: `PILOT_EFFECT_TEST_CRASH=before_record|after_record|torn`（最初の
  publish でその地点で process を落とす）、`PILOT_EFFECT_TEST_GATE=<dir>`（要求を受けたら `arrived.<n>`、lock の
  中で記録を読んだら `inside.<n>` を置き、`go` が置かれるまで待つ）。
"""
import argparse
import fcntl
import hashlib
import hmac
import json
import os
import pathlib
import socket
import socketserver
import stat
import sys
import threading
import time
import urllib.error
import urllib.request

EXIT_OK, EXIT_USAGE, EXIT_UNAPPROVED, EXIT_REJECTED, EXIT_UNAVAILABLE = 0, 2, 3, 4, 5
MARKER = "pilot-effect:"
GENESIS = "0" * 64


def sock_address(name):
    """Linux の abstract socket（path を持たない。worker の mount namespace・読み取り専用の mount に左右されない）。"""
    return "\0" + name


# ---- 記録（HMAC の鎖） ----

def mac(key, prev, body):
    return hmac.new(key, (prev + "\n" + json.dumps(body, sort_keys=True, ensure_ascii=False)).encode(),
                    hashlib.sha256).hexdigest()


class ChainError(Exception):
    pass


def read_chain(path, key):
    """鎖を検証して行（mac・prev を除いた body）と末尾の MAC を返す。欠け・改ざん・順序の入れ替えは ChainError。"""
    rows, macs = read_chain_macs(path, key)
    return rows, (macs[-1] if macs else GENESIS)


def read_chain_macs(path, key):
    rows = []
    macs = []
    prev = GENESIS
    data = pathlib.Path(path).read_text(encoding="utf-8")
    if data and not data.endswith("\n"):
        raise ChainError(f"{path}: the last line is incomplete")
    for n, line in enumerate(data.splitlines(), 1):
        try:
            row = json.loads(line)
        except ValueError as e:
            raise ChainError(f"{path}:{n}: not JSON ({e})") from None
        got = row.pop("mac", None)
        if row.pop("prev", None) != prev:
            raise ChainError(f"{path}:{n}: prev does not match")
        if row.get("seq") != n:
            raise ChainError(f"{path}:{n}: seq {row.get('seq')}")
        want = mac(key, prev, row)
        if not isinstance(got, str) or not hmac.compare_digest(got, want):
            raise ChainError(f"{path}:{n}: MAC does not match")
        rows.append(row)
        macs.append(want)
        prev = want
    return rows, macs


def verify(directory, key, head=None):
    """driver の collector が使う。(effects, audit) を返す。

    記録が無い・読めない・鎖が切れている・audit の `published` と効果の行が 1 対 1 でない・broker が知らせた末尾
    （`head`: {"effects": [seq, mac], "audit": [seq, mac]}）が file に無い（末尾を切った）ときは ChainError。
    効果の追記の直後に broker が落ちて audit の行が無い 1 件は認める（次の起動で broker が audit を補う）。"""
    d = pathlib.Path(directory)
    chains = {}
    for name in ("effects", "audit"):
        p = d / f"{name}.jsonl"
        if not p.is_file():
            raise ChainError(f"{p}: missing")
        chains[name] = read_chain_macs(p, key)
    (effects, effect_macs), (audit, _) = chains["effects"], chains["audit"]
    published = [a.get("effect_mac") for a in audit if a.get("result") == "published"]
    if published != effect_macs[: len(published)] or len(effect_macs) - len(published) > 1:
        raise ChainError("audit `published` rows and effect rows do not match")
    for name, (seq, want) in (head or {}).items():
        macs = chains[name][1]
        if seq > len(macs) or (seq > 0 and macs[seq - 1] != want):
            raise ChainError(f"{name}.jsonl is shorter than the broker reported (seq {seq})")
    return effects, audit


class Record:
    """broker だけが持つ。追記の前に鎖の末尾を検証し、改ざんがあれば以後の書き込みを断る。"""

    def __init__(self, directory, key):
        self.dir = pathlib.Path(directory)
        self.key = key
        self.lock = threading.Lock()
        self.dir.mkdir(parents=True, exist_ok=True)
        with self.writable():
            for name in ("effects.jsonl", "audit.jsonl"):
                p = self.dir / name
                if not p.exists():
                    p.write_text("")
            self.recover()

    class _Open:
        def __init__(self, rec):
            self.rec = rec

        def __enter__(self):
            os.chmod(self.rec.dir, 0o700)
            for name in ("effects.jsonl", "audit.jsonl"):
                p = self.rec.dir / name
                if p.exists():
                    os.chmod(p, 0o600)
            return self

        def __exit__(self, *exc):
            for name in ("effects.jsonl", "audit.jsonl"):
                p = self.rec.dir / name
                if p.exists():
                    os.chmod(p, 0o444)
            os.chmod(self.rec.dir, 0o555)
            return False

    def writable(self):
        return Record._Open(self)

    def recover(self):
        """追記の途中で落ちた末尾（改行の無い行）を切り詰め、audit に残す。それ以外の不整合は直さない。"""
        for name in ("effects.jsonl", "audit.jsonl"):
            p = self.dir / name
            data = p.read_bytes()
            if data and not data.endswith(b"\n"):
                cut = data.rfind(b"\n") + 1
                with open(p, "r+b") as f:
                    f.truncate(cut)
                    f.flush()
                    os.fsync(f.fileno())
                self.audit({"event": "recovered_torn_tail", "file": name,
                            "dropped_bytes": len(data) - cut})
        # 効果の追記の直後に落ちた（audit の行が無い）なら、audit に published を補う（効果は増やさない）。
        effects, macs = read_chain_macs(self.dir / "effects.jsonl", self.key)
        audit = read_chain(self.dir / "audit.jsonl", self.key)[0]
        done = [a.get("effect_mac") for a in audit if a.get("result") == "published"]
        if len(macs) == len(done) + 1 and done == macs[: len(done)]:
            e = effects[-1]
            self.audit({"event": "publish", "logical_id": e["logical_id"], "result": "published",
                        "approval_id": e.get("approval_id"), "decision": e.get("decision"),
                        "effect_mac": macs[-1], "recovered": True})

    def head(self):
        out = {}
        for name in ("effects", "audit"):
            _, macs = read_chain_macs(self.dir / f"{name}.jsonl", self.key)
            out[name] = [len(macs), macs[-1] if macs else GENESIS]
        return out

    def _append(self, name, body, crash=None):
        p = self.dir / name
        _, prev = read_chain(p, self.key)
        seq = sum(1 for _ in p.read_text(encoding="utf-8").splitlines()) + 1
        row = {"seq": seq, "at": time.time(), **body}
        line = json.dumps({**row, "prev": prev, "mac": mac(self.key, prev, row)}, ensure_ascii=False) + "\n"
        raw = line.encode()
        fd = os.open(p, os.O_WRONLY | os.O_APPEND)
        try:
            if crash == "torn":
                os.write(fd, raw[: len(raw) // 2])
                os.fsync(fd)
                os._exit(9)
            os.write(fd, raw)
            os.fsync(fd)
        finally:
            os.close(fd)
        return {**row, "mac": mac(self.key, prev, row)}

    def effects(self):
        return read_chain(self.dir / "effects.jsonl", self.key)[0]

    def audit(self, body):
        return self._append("audit.jsonl", body)

    def effect(self, body, crash=None):
        return self._append("effects.jsonl", body, crash)


# ---- 承認（daemon API） ----

class DaemonApprovals:
    def __init__(self, base, token, project):
        self.base = base.rstrip("/")
        self.token = token
        self.project = project

    def get(self, path):
        req = urllib.request.Request(self.base + path, headers={"Authorization": f"Bearer {self.token}"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read() or b"null")

    def human_answered(self, task_id, question):
        """task の `Answered` event のうち、この question に人（`human:`）が答えたものがあるか。"""
        events = self.get(f"/api/v1/tasks/{task_id}/events?types=answered&limit=5000")
        items = events.get("items", events) if isinstance(events, dict) else events
        for item in items or []:
            ev = item.get("event", item) if isinstance(item, dict) else {}
            if isinstance(ev, dict) and ev.get("type") == "answered" and ev.get("question") == question \
                    and str(ev.get("by") or "").startswith("human:"):
                return True
        return False

    def decision(self, logical_id):
        """(decision, approval) を返す。decision は approved / rejected / none。"""
        items = self.get(f"/api/v1/approvals?project={self.project}")["items"]
        mine = [a for a in items if (MARKER + logical_id) in a.get("question", "")]
        decided = [a for a in mine if a.get("decision") in ("once", "standing", "denied")]
        decided.sort(key=lambda a: (a.get("decided_at") or "", a["id"]))
        for a in reversed(decided):
            if a.get("task_id") and self.human_answered(a["task_id"], a["question"]):
                return ("rejected" if a["decision"] == "denied" else "approved"), a
        return "none", (mine[-1] if mine else None)


# ---- broker ----

class Broker:
    def __init__(self, record, approvals, report=None):
        self.record = record
        self.report = report or (lambda head: None)
        self.approvals = approvals
        self.crash = os.environ.get("PILOT_EFFECT_TEST_CRASH") or None
        self.gate = os.environ.get("PILOT_EFFECT_TEST_GATE") or None
        self.arrivals = 0
        self.count_lock = threading.Lock()
        self.flock_path = record.dir.parent / (record.dir.name + ".lock")

    def _gate(self, kind, n):
        if not self.gate:
            return
        g = pathlib.Path(self.gate)
        (g / f"{kind}.{n}").write_text("")
        if kind == "inside":
            while not (g / "go").exists():
                time.sleep(0.05)

    def publish(self, logical_id, peer):
        try:
            return self._publish(logical_id, peer)
        finally:
            try:
                self.report(self.record.head())
            except (ChainError, OSError):
                pass

    def _publish(self, logical_id, peer):
        with self.count_lock:
            self.arrivals += 1
            n = self.arrivals
        self._gate("arrived", n)
        with self.record.lock, open(self.flock_path, "a") as lockf:
            fcntl.flock(lockf, fcntl.LOCK_EX)
            with self.record.writable():
                done = {e["logical_id"] for e in self.record.effects()}
                self._gate("inside", n)
                base = {"event": "publish", "logical_id": logical_id, "peer": peer}
                if logical_id in done:
                    self.record.audit({**base, "result": "duplicate"})
                    return EXIT_OK, "already published (recorded once)"
                try:
                    verdict, approval = self.approvals.decision(logical_id)
                except (urllib.error.URLError, OSError, ValueError, KeyError) as e:
                    self.record.audit({**base, "result": "daemon_unavailable", "error": str(e)[:200]})
                    return EXIT_UNAVAILABLE, f"cannot read approvals from the daemon: {e}"
                ref = {"approval_id": approval and approval.get("id"),
                       "decision": approval and approval.get("decision")}
                if verdict == "rejected":
                    self.record.audit({**base, **ref, "result": "rejected"})
                    return EXIT_REJECTED, "the publication was rejected by a person; do not publish"
                if verdict != "approved":
                    self.record.audit({**base, **ref, "result": "refused_unapproved"})
                    return EXIT_UNAPPROVED, (f"approval required: ask a person with a question that contains "
                                             f"`{MARKER}{logical_id}` and stop; run this command again after the answer")
                crash, self.crash = self.crash, None
                if crash == "before_record":
                    os._exit(9)
                effect = self.record.effect({"logical_id": logical_id, "approved": True, **ref}, crash)
                if crash == "after_record":
                    os._exit(9)
                self.record.audit({**base, **ref, "result": "published", "effect_mac": effect["mac"]})
                return EXIT_OK, "published (pilot stub: no external publication)"


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        try:
            req = json.loads(self.rfile.readline(65536) or b"{}")
            if req.get("verb") != "publish" or not isinstance(req.get("logical_id"), str) \
                    or not req["logical_id"] or len(req["logical_id"]) > 200:
                code, msg = EXIT_USAGE, "usage: publish <logical_id>"
            else:
                peer = {"pid": req.get("pid")}
                code, msg = self.server.broker.publish(req["logical_id"], peer)
        except ChainError as e:
            code, msg = EXIT_UNAVAILABLE, f"the effect record is broken: {e}"
        self.wfile.write((json.dumps({"exit": code, "message": msg}) + "\n").encode())


class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True


def serve(a):
    key = bytes.fromhex(sys.stdin.readline().strip())
    if len(key) < 32:
        sys.exit("serve: a 32-byte hex key is required on stdin")
    token = pathlib.Path(a.token_file).read_text().strip()
    record = Record(a.dir, key)
    srv = Server(sock_address(a.socket), Handler)
    out_lock = threading.Lock()

    def report(head):
        with out_lock:
            print("head " + json.dumps(head), flush=True)

    srv.broker = Broker(record, DaemonApprovals(a.api, token, a.project), report)
    print("ready", flush=True)
    report(record.head())
    srv.serve_forever()


class HeadReader:
    """broker の stdout（`head` の行）を読む thread。driver と試験が、broker の知らせた最後の末尾を持つ。"""

    def __init__(self, stream):
        self.head = None
        self.stream = stream
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        for line in self.stream:
            if line.startswith("head "):
                self.head = json.loads(line[5:])

    def wait(self, timeout=30):
        """broker が終わる（stdout が閉じる）まで待って、最後の末尾を返す。"""
        self.thread.join(timeout)
        return self.head


# ---- client ----

def publish_client(name, logical_id, timeout):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(sock_address(name))
        s.sendall((json.dumps({"verb": "publish", "logical_id": logical_id, "pid": os.getpid()}) + "\n").encode())
        data = b""
        while not data.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            data += chunk
    except OSError as e:
        print(f"pilot-effect: the effect broker is not reachable: {e}", file=sys.stderr)
        return EXIT_UNAVAILABLE
    finally:
        s.close()
    try:
        resp = json.loads(data)
    except ValueError:
        print("pilot-effect: the effect broker closed the connection", file=sys.stderr)
        return EXIT_UNAVAILABLE
    print(f"pilot-effect: {resp['message']}", file=sys.stderr if resp["exit"] else sys.stdout)
    return int(resp["exit"])


def write_wrapper(path, socket_name):
    """worker に公開する command（socket 名と client の場所を埋めた wrapper）。記録の場所は含まない。"""
    me = pathlib.Path(__file__).resolve()
    p = pathlib.Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("#!/bin/sh\n# pilot の公開 command（記録は worker の外の broker が持つ）。\n"
                 "[ \"$1\" = publish ] && [ $# -eq 2 ] || { echo 'usage: pilot-effect publish <logical_id>' >&2; exit 2; }\n"
                 f"exec python3 -I {json.dumps(str(me))} publish \"$2\" --socket {json.dumps(socket_name)}\n")
    p.chmod(stat.S_IRUSR | stat.S_IXUSR | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
    return p


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve")
    s.add_argument("--dir", required=True)
    s.add_argument("--socket", required=True)
    s.add_argument("--api", required=True)
    s.add_argument("--token-file", required=True)
    s.add_argument("--project", required=True)
    c = sub.add_parser("publish")
    c.add_argument("logical_id")
    c.add_argument("--socket", required=True)
    c.add_argument("--timeout", type=float, default=120)
    a = p.parse_args()
    if a.cmd == "serve":
        serve(a)
    else:
        sys.exit(publish_client(a.socket, a.logical_id, a.timeout))


if __name__ == "__main__":
    main()
