#!/usr/bin/env bash
# ADR 2026-10-09-browser-launcher-credential-release: sd_browser_ledger の launcher credential 段（lib.sh の 3b）。
#   (1) launcher socket が無い host: 台帳は置かれ（ok）、credential は空（credential_backends == []）、古い credential claim は
#       落ち、理由が ledger-status.json と log に残る。sd_browser_ledger は 0 を返す（release は止めない）。
#   (2) launcher socket がある host: 生成器が --credential-runtime launcher と socket 付きで呼ばれ、credential 証拠が採られる。
# 偽の生成器・agent-browser・celerisctl だけを使う。外部ネットワーク・userns・本番 host（/run・~/.config・~/.local）には触れない。
[ -n "${BASH_VERSION:-}" ] || exec bash "$0" "$@"
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/state" "$root/config" "$root/build"
: >"$root/fake.log"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/case.log" ] && tail -n 30 "$root/case.log" >&2
  exit 1
}

# `json_is <file> <python expr over d>`
json_is() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
sys.exit(0 if eval(sys.argv[2], {"d": d}) else 1)
PY
}

# ---- 偽の道具 ----------------------------------------------------------------

cat >"$root/bin/agent-browser" <<'EOF'
#!/usr/bin/env bash
echo "agent-browser 0.38.1"
EOF

# `browser ledger check --file <ledger>`: 台帳の backend と、isolation_suite が passed の backend を credential として返す。
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
file=""
while [ $# -gt 0 ]; do [ "$1" = --file ] && file="$2"; shift; done
python3 - "$file" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(json.dumps({
    "ok": True, "code": "ok",
    "backends": [r["backend_id"] for r in d["results"]],
    "credential_backends": [r["backend_id"] for r in d["results"] if "isolation_suite" in r.get("passed", [])],
}))
PY
EOF

# 偽の生成器（browser-conformance.py の代わり）。protocol pass は公開台帳を書く（FAKE_STALE=1 なら古い credential claim を
# 混ぜる）。--credential-evidence は CELERIS_BROWSER_LAUNCHER_SOCKET の有無で launcher_unavailable（claim を消す・exit 1）か
# ok（claim を足す・exit 0）を返す。launcher 以外の runtime は拒否する。
cat >"$root/bin/fake-runner" <<'EOF'
#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]


def opt(name, default=None):
    return args[args.index(name) + 1] if name in args else default


with open(os.environ["FAKE_LOG"], "a", encoding="utf-8") as fh:
    socket = os.environ.get("CELERIS_BROWSER_LAUNCHER_SOCKET")
    fh.write("runner " + " ".join(args) + f" socket={socket if socket is not None else '<unset>'}\n")

out = opt("--output-dir")
os.makedirs(out, exist_ok=True)

if "--p4b-evidence" in args:
    sys.exit(0)

if "--credential-evidence" not in args:
    passed = ["public_capability"] + (["isolation_suite"] if os.environ.get("FAKE_STALE") == "1" else [])
    ledger = {"schema": 1, "source": "celeris-browser-conformance-protocol-scripted",
              "generated_for": {"celeris_release": opt("--celeris-release", "")},
              "results": [{"backend_id": "claude-code", "passed": list(passed), "evidence": []},
                          {"backend_id": "browser-specialist", "passed": ["public_capability"], "evidence": []}]}
    with open(os.path.join(out, "conformance.json"), "w", encoding="utf-8") as fh:
        json.dump(ledger, fh)
    sys.exit(0)

if opt("--credential-runtime", "daemon") != "launcher":
    sys.exit(9)
backends = [args[i + 1] for i, a in enumerate(args) if a == "--credential-backend"]
ledger = json.load(open(opt("--credential-evidence"), encoding="utf-8"))
if not os.environ.get("CELERIS_BROWSER_LAUNCHER_SOCKET"):
    for r in ledger["results"]:
        if r["backend_id"] in backends:
            r["passed"] = [p for p in r["passed"] if p not in ("isolation_suite", "egress_negative_suite")]
    report = {"complete": False, "code": "launcher_unavailable",
              "reason": "CELERIS_BROWSER_LAUNCHER_SOCKET is not configured", "evidence": []}
    code = 1
else:
    for r in ledger["results"]:
        if r["backend_id"] in backends:
            r["passed"] = sorted(set(r["passed"]) | {"isolation_suite", "egress_negative_suite"})
    report = {"complete": True, "code": "ok", "reason": "", "evidence": []}
    code = 0
with open(os.path.join(out, "conformance.json"), "w", encoding="utf-8") as fh:
    json.dump(ledger, fh)
with open(os.path.join(out, "credential-evidence.json"), "w", encoding="utf-8") as fh:
    json.dump(report, fh)
sys.exit(code)
EOF
chmod +x "$root/bin/"*

# launcher socket の代わり（存在だけが要る）。TMPDIR が長いと AF_UNIX の path 上限に当たるので、相対 path で作る。
(cd "$root" && python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])' launcher.sock)

export CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config"
export FAKE_LOG="$root/fake.log"
# shellcheck source=../lib.sh
. "$here/lib.sh"
SD_BROWSER_LEDGER_RUNNER="$root/bin/fake-runner"
SD_BROWSER_LEDGER_CHECK_BIN="$root/bin/celerisctl"
SD_AGENT_BROWSER="$root/bin/agent-browser"
SD_BROWSER_LEDGER_TIMEOUT=120
export SD_BROWSER_LEDGER_RUNNER SD_BROWSER_LEDGER_CHECK_BIN SD_AGENT_BROWSER SD_BROWSER_LEDGER_TIMEOUT

# ---- (1) launcher socket が無い host ------------------------------------------------

out1="$root/cases/no-launcher"
mkdir -p "$out1"
rc=0
FAKE_STALE=1 SD_BROWSER_LAUNCHER_SOCKET="" sd_browser_ledger "$root/build" "$out1" 0123456789ab >"$root/case.log" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "release-side ledger failed without a launcher (rc=$rc)"
[ "$SD_BROWSER_LEDGER_OK" = true ] || fail "ledger was not placed without a launcher"
[ -f "$out1/browser/conformance.json" ] || fail "public ledger was not placed without a launcher"
json_is "$out1/browser/ledger-status.json" \
  "d['ok'] is True and d['credential_evidence'] == {'ok': False, 'code': 'launcher_unavailable', 'reason': 'CELERIS_BROWSER_LAUNCHER_SOCKET is not configured'} and d['credential_backends'] == []" \
  || fail "ledger-status does not record launcher_unavailable with the reason and empty credential_backends"
json_is "$out1/browser/conformance.json" \
  "all('isolation_suite' not in r.get('passed', []) for r in d['results'])" \
  || fail "stale credential claim survived without a launcher"
grep -q 'credential evidence exit 1 (code=launcher_unavailable): CELERIS_BROWSER_LAUNCHER_SOCKET is not configured' "$root/case.log" \
  || fail "log does not record the launcher_unavailable reason"
grep -q -- '--credential-runtime launcher' "$root/fake.log" || fail "credential evidence did not request the launcher runtime"

# ---- (2) launcher socket がある host -----------------------------------------------

out2="$root/cases/launcher"
mkdir -p "$out2"
rc=0
FAKE_STALE=0 SD_BROWSER_LAUNCHER_SOCKET="$root/launcher.sock" sd_browser_ledger "$root/build" "$out2" 0123456789ab >"$root/case.log" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "release-side ledger failed with a launcher (rc=$rc)"
json_is "$out2/browser/ledger-status.json" \
  "d['ok'] is True and d['credential_evidence'] == {'ok': True, 'code': 'ok', 'reason': ''} and d['credential_backends'] == ['claude-code', 'browser-specialist']" \
  || fail "ledger-status does not record launcher credential evidence"
grep -q -- "--credential-runtime launcher .*socket=$root/launcher.sock" "$root/fake.log" \
  || fail "generator did not receive the launcher runtime and socket"

echo "browser_ledger_launcher_credential: ok (no-launcher: launcher_unavailable recorded; launcher: credential evidence adopted)"
