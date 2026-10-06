#!/usr/bin/env python3
"""Diagnostic observer (Fable rerun 3), run as root via sudo -n. Read-only: uses inotify to copy each
launcher action request/result (the agent-browser JSON reply) for the test launcher's sessions under
/var/tmp/cb-1001/launcher-state/sessions into one JSONL. It never writes into the session dir."""
import ctypes, json, os, struct, sys, time
SESS = "/var/tmp/cb-1001/launcher-state/sessions"
OUT = sys.argv[1]
libc = ctypes.CDLL("libc.so.6", use_errno=True)
IN_CREATE, IN_CLOSE_WRITE, IN_MOVED_TO, IN_ISDIR = 0x100, 0x8, 0x80, 0x40000000
fd = libc.inotify_init()
watches = {}
def add(path, mask):
    wd = libc.inotify_add_watch(fd, path.encode(), mask)
    if wd >= 0: watches[wd] = path
    return wd
def emit(rec):
    rec["t"] = round(time.time(), 3)
    with open(OUT, "a") as f: f.write(json.dumps(rec) + "\n")
add(SESS, IN_CREATE)
for s in os.listdir(SESS):
    a = os.path.join(SESS, s, "actions")
    if os.path.isdir(a): add(a, IN_CLOSE_WRITE | IN_MOVED_TO)
emit({"observer": "started"})
while True:
    buf = os.read(fd, 65536)
    i = 0
    while i < len(buf):
        wd, mask, _, ln = struct.unpack_from("iIII", buf, i)
        name = buf[i+16:i+16+ln].rstrip(b"\0").decode()
        i += 16 + ln
        base = watches.get(wd)
        if base is None: continue
        if base == SESS and mask & IN_ISDIR:
            a = os.path.join(SESS, name, "actions")
            for _ in range(500):
                if os.path.isdir(a): break
                time.sleep(0.01)
            add(a, IN_CLOSE_WRITE | IN_MOVED_TO)
            emit({"session_dir": name})
        elif name.endswith((".request", ".result")):
            try: body = open(os.path.join(base, name)).read()
            except OSError as e: body = f"<unreadable: {e.strerror}>"
            emit({"session": os.path.basename(os.path.dirname(base)), "file": name, "body": body[:4000]})
