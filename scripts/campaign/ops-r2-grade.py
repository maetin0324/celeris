#!/usr/bin/env python3
"""ops-r2 の採点 wrapper（/var/lib/celeris-eval-bin/grade として celeris-eval 持ち 0555 で置く）。

drop box の tgz を sha256 で照合して EVAL_ROOT/work/ に展開し、celeris-campaign を
--grader-root EVAL_ROOT/private で走らせ、判定 JSON を check-ops-r1-leak.py に通してから
outbox に 0644 で置く（docs/ops/ops-r2-isolation.md §4.3、ADR 2026-10-30-ops-r2-isolation-principal D4）。

stdout に出すのは outbox の判定 JSON の path だけ。失敗は短い理由だけを stderr に出す。
private の中身・EVAL_ROOT の path 一覧・celeris-campaign の出力は stdout/stderr・outbox に出さない
（celeris-campaign と漏洩検査の出力は EVAL_ROOT/work/ の中にだけ残す）。

usage: grade --tgz <drop box の tgz> --sha256 <hex> --case <case id> -- <celeris-campaign の引数...>
  引数の {work} は展開先の dir、{verdict} は判定 JSON の書き先に置き換える（{verdict} は必須）。
  --grader-root は wrapper が付ける。引数に --grader-root・'..'・work の外の絶対 path は書けない。
"""
import hashlib
import os
import re
import stat
import subprocess
import sys
import tarfile
import tempfile

EVAL_ROOT = "/var/lib/celeris-eval"
BIN_DIR = "/var/lib/celeris-eval-bin"
OUTBOX = "/var/lib/celeris-eval-outbox"
CASE_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,127}$")
HEX_RE = re.compile(r"^[0-9a-f]{64}$")


def fail(code, msg):
    sys.stderr.write(f"grade: {msg}\n")
    sys.exit(code)


def parse(argv):
    opts = {"--tgz": None, "--sha256": None, "--case": None}
    rest = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--":
            rest = argv[i + 1:]
            break
        if a in opts and i + 1 < len(argv):
            opts[a] = argv[i + 1]
            i += 2
            continue
        fail(64, f"unknown argument: {a}")
    if not all(opts.values()) or not rest:
        fail(64, "usage: grade --tgz <tgz> --sha256 <hex> --case <id> -- <celeris-campaign args>")
    return opts["--tgz"], opts["--sha256"].lower(), opts["--case"], rest


def euid():
    # 試験だけが別 uid を模すために差し替える（実機では使わない）。
    v = os.environ.get("CELERIS_EVAL_TEST_EUID")
    return int(v) if v is not None else os.geteuid()


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def safe_extract(tgz, dest):
    with tarfile.open(tgz, "r:gz") as t:
        for m in t.getmembers():
            n = m.name
            if n.startswith("/") or ".." in n.split("/"):
                fail(65, "tgz has an unsafe member path")
            if not (m.isfile() or m.isdir()):
                fail(65, "tgz has a non-regular member")
        t.extractall(dest, filter="data")


def main(argv):
    tgz, want, case, rest = parse(argv)
    eval_root = os.environ.get("CELERIS_EVAL_ROOT", EVAL_ROOT)
    bin_dir = os.environ.get("CELERIS_EVAL_BIN", BIN_DIR)
    outbox = os.environ.get("CELERIS_EVAL_OUTBOX", OUTBOX)
    if not CASE_RE.match(case):
        fail(64, "invalid case id")
    if not HEX_RE.match(want):
        fail(64, "--sha256 must be 64 hex chars")
    st = os.stat(eval_root)
    if euid() != st.st_uid:
        fail(77, "must run as the owner of EVAL_ROOT (celeris-eval)")
    if stat.S_IMODE(st.st_mode) & 0o077:
        fail(77, "EVAL_ROOT must not be accessible by group/other")
    try:
        got = sha256_file(tgz)
    except OSError:
        fail(66, "cannot read tgz")
    if got != want:
        fail(65, "tgz sha256 mismatch")

    os.makedirs(os.path.join(eval_root, "work"), mode=0o700, exist_ok=True)
    work = tempfile.mkdtemp(prefix=f"{case}-{got[:12]}-", dir=os.path.join(eval_root, "work"))
    art = os.path.join(work, "artifact")
    os.mkdir(art, 0o700)
    safe_extract(tgz, art)
    verdict = os.path.join(work, "verdict.json")

    args = []
    seen_verdict = False
    for a in rest:
        if "{verdict}" in a:
            seen_verdict = True
        a = a.replace("{work}", art).replace("{verdict}", verdict)
        if a == "--grader-root" or a.startswith("--grader-root="):
            fail(64, "--grader-root is set by the wrapper")
        if ".." in a.split("/"):
            fail(64, "argument must not contain '..'")
        if a.startswith("/") and not (a == work or a.startswith(work + "/")):
            fail(64, "absolute paths outside the work dir are not allowed")
        args.append(a)
    if not seen_verdict:
        fail(64, "celeris-campaign arguments must contain {verdict}")

    private_root = os.path.join(eval_root, "private")
    cc = [os.path.join(bin_dir, "celeris-campaign"), *args, "--grader-root", private_root]
    with open(os.path.join(work, "celeris-campaign.log"), "wb") as log:
        r = subprocess.run(cc, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, cwd=work)
    if r.returncode != 0 or not os.path.isfile(verdict):
        fail(70, f"celeris-campaign failed (exit {r.returncode}); log is kept under EVAL_ROOT/work")

    private = os.path.join(private_root, "final", case, "private.json")
    leak = [sys.executable, "-I", os.path.join(bin_dir, "check-ops-r1-leak.py"),
            "--private", private, "--scan", verdict, "--out", os.path.join(work, "leak.json")]
    with open(os.path.join(work, "leak.log"), "wb") as log:
        r = subprocess.run(leak, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, cwd=work)
    if r.returncode != 0:
        fail(71, f"leak check did not pass (exit {r.returncode}); verdict was not published")

    with open(verdict, "rb") as f:
        body = f.read()
    name = f"{case}-{got[:16]}.verdict.json"
    fd, tmp = tempfile.mkstemp(prefix=".grade-", dir=outbox)
    with os.fdopen(fd, "wb") as f:
        f.write(body)
    os.chmod(tmp, 0o644)
    os.replace(tmp, os.path.join(outbox, name))
    print(os.path.join(outbox, name))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
