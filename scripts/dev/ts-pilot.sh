#!/usr/bin/env bash
# 自己改善 pilot（境界 ADR 2026-11-02-ts-control-layer-boundary 付記 S5・S6）: TS 制御コードの方針を 1 つ改善し、
# 検証 → 候補 bundle → 試験用の一時 DB の daemon に配備・generation 登録 → compare → canary → 差分の記録 →
# rust への rollback → 再起動後の復元確認 を 1 件通し、各段の時刻・秒・event・actor を record.jsonl に書く。
#   bash scripts/dev/ts-pilot.sh --out <dir>                 pilot を 1 件走らせ、<dir>/record.jsonl・candidate.patch・
#                                                            ts-verify.json を書く（<dir> の既存の record.jsonl は上書き）
#   bash scripts/dev/ts-pilot.sh --validate <record.jsonl>   記録を検査する（欠け・食い違いがあれば exit 1）
#
# P2-G（ADR 2026-11-03-llm-self-improve-trial）の環境変数（どちらも任意。無ければ S6 pilot の固定の改善のまま）:
#   TS_PILOT_PATCH=<file>   固定の改善の代わりに候補の patch（control-ts/src/ の中だけ）を当てる。外の file を変える patch は失敗
#   TS_PILOT_SPEC=<spec>    課題の改善仕様（同じ dir の <名>.cases.jsonl と組）。試験 daemon で人でない主体の承認が 403 で拒まれ、
#                           人の token の承認が通ることを記録し、段 diff の probe を期待差分 fixture の入力にする
#   TS_PILOT_CHANGE=<text>  段 edit の change の説明
# 守ること:
# - この作業ツリーの control-ts/src は変えない。改善は $TMPDIR の使い捨ての作業ツリー（git worktree --detach HEAD）に入れ、
#   差分は <dir>/candidate.patch にだけ残す。
# - 本番の mode・deploy・state dir・DB・config に触れない。daemon は $TMPDIR の一時 DB・一時 state dir・loopback の port で
#   起こし、終わったら止めて消す。外部ネットワークに出ない。LLM を呼ばない（llm source は `fake`、CoS は無効）。
# - 評価器・hard safety gate を弱めない。ts-verify.sh はそのまま走らせる。canary の前提（(f) の進む条件）を満たす計数は、
#   gen-e2e（crates/celeris/tests/script_generation_e2e.rs の observe_until_ready）と同じく一時 DB に「成功の呼び出し」だけを
#   積む。その段は記録に synthetic として残し、本番では compare の実 pass が 200 件要ることを書く。
# - 言語移行の測定（edit-activate-baseline.sh）とは別の記録（kind = policy_improvement）にする。
# exit: 0 = 成功、1 = 段の失敗・検査の失敗、2 = 使い方の誤り。
set -euo pipefail

usage() { echo "usage: ts-pilot.sh --out <dir> | --validate <record.jsonl>" >&2; exit 2; }
[ "$#" -eq 2 ] || usage
case "$1" in
  --out|--validate) ;;
  *) usage ;;
esac
cmd="$1"
arg="$2"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---- 記録の検査 ----
validate() {
  python3 -I - "$1" <<'PY'
import json, sys
from datetime import datetime

path = sys.argv[1]
errors = []
try:
    lines = [l for l in open(path, encoding="utf-8") if l.strip()]
except OSError as e:
    print(f"ts-pilot: 記録を読めない: {e}", file=sys.stderr)
    sys.exit(1)

REQUIRED = [
    "meta", "edit", "ts_verify", "bundle", "daemon_start", "deploy", "register", "compare",
    "canary_denied_non_human", "canary_not_ready", "guard_seed", "canary", "diff",
    "restart_restore_canary", "rollback", "restart_restore_rust",
]
KEYS = ["schema", "seq", "stage", "started_at", "ended_at", "secs", "actor", "ok", "events", "detail"]

def ts(v, where):
    try:
        return datetime.fromisoformat(v.replace("Z", "+00:00"))
    except Exception:
        errors.append(f"{where}: 時刻が読めない: {v!r}")
        return None

stages = {}
prev_seq = 0
for n, line in enumerate(lines, 1):
    try:
        r = json.loads(line)
    except json.JSONDecodeError as e:
        errors.append(f"line {n}: JSON でない: {e}")
        continue
    where = f"line {n} ({r.get('stage')})"
    missing = [k for k in KEYS if k not in r]
    if missing:
        errors.append(f"{where}: 欄が無い: {missing}")
        continue
    if r["schema"] != "celeris.ts-pilot/1":
        errors.append(f"{where}: schema が違う: {r['schema']}")
    if not isinstance(r["seq"], int) or r["seq"] != prev_seq + 1:
        errors.append(f"{where}: seq が 1 から連続していない: {r['seq']}")
    prev_seq = r["seq"] if isinstance(r["seq"], int) else prev_seq
    a, b = ts(r["started_at"], where), ts(r["ended_at"], where)
    if not isinstance(r["secs"], (int, float)) or r["secs"] < 0:
        errors.append(f"{where}: secs が負か数でない")
    elif a and b and abs((b - a).total_seconds() - r["secs"]) > 1.0:
        errors.append(f"{where}: secs が started_at・ended_at と合わない")
    if not isinstance(r["actor"], str) or not r["actor"]:
        errors.append(f"{where}: actor が空")
    if r["ok"] is not True:
        errors.append(f"{where}: ok でない")
    if not isinstance(r["events"], list) or not isinstance(r["detail"], dict):
        errors.append(f"{where}: events は配列、detail は object")
    if r["stage"] in stages:
        errors.append(f"{where}: 段が重複している")
    stages[r["stage"]] = r

for s in REQUIRED:
    if s not in stages:
        errors.append(f"段が無い: {s}")
order = [s for s in REQUIRED if s in stages]
seqs = [stages[s]["seq"] for s in order]
if seqs != sorted(seqs):
    errors.append(f"段の順が違う: {order}")

def need(cond, msg):
    if not cond:
        errors.append(msg)

g = stages.get
if g("meta"):
    d = g("meta")["detail"]
    need(d.get("kind") == "policy_improvement", "meta: kind が policy_improvement でない")
    need(d.get("production_touched") is False, "meta: production_touched が false でない")
    need(isinstance(d.get("commit"), str) and len(d.get("commit", "")) == 40, "meta: commit が無い")
    need("llm_usage" in d, "meta: llm_usage が無い")
if g("edit"):
    d = g("edit")["detail"]
    need(d.get("patch_sha256") and d.get("files"), "edit: patch_sha256・files が無い")
    need(all(f.startswith("control-ts/src/") for f in d.get("files", [])), "edit: control-ts/src の外の file を変えた")
if g("ts_verify"):
    d = g("ts_verify")["detail"]
    need(d.get("exit") == 0 and d.get("class") == "ts-only", "ts_verify: exit 0・ts-only でない")
    need({"typecheck", "bundle", "unit", "golden", "safety", "integration", "activate"} <= set(d.get("stages", {})),
         "ts_verify: 段が欠けた")
if g("bundle") and g("register"):
    sha = g("bundle")["detail"].get("bundle_sha256", "")
    need(len(sha) == 64, "bundle: bundle_sha256 が無い")
    need(g("deploy")["detail"].get("bundle_sha256") == sha if g("deploy") else False, "deploy: hash が bundle と違う")
    need(g("register")["detail"].get("bundle_sha256") == sha, "register: hash が bundle と違う")
for s, status in [("register", 200), ("compare", 200), ("canary_denied_non_human", 403),
                  ("canary_not_ready", 422), ("canary", 200), ("rollback", 200)]:
    if g(s):
        need(g(s)["detail"].get("http_status") == status, f"{s}: http_status が {status} でない")
for s in ["register", "compare", "canary", "rollback"]:
    if g(s):
        need(len(g(s)["events"]) > 0, f"{s}: event が無い")
        for e in g(s)["events"]:
            need({"table", "id", "type", "actor"} <= set(e), f"{s}: event の欄が欠けた: {e}")
if g("compare"):
    need(not g("compare")["actor"].startswith("human:"), "compare: 人でない主体（CoS 相当）で行っていない")
    need(any(e.get("to_mode") == "compare" for e in g("compare")["events"]), "compare: mode の変更の event が無い")
if g("canary_denied_non_human"):
    need(g("canary_denied_non_human")["detail"].get("error") == "script_mode_requires_human",
         "canary_denied_non_human: error が script_mode_requires_human でない")
    need(g("canary_denied_non_human")["events"] == [], "canary_denied_non_human: 拒否で event が増えた")
if g("canary_not_ready"):
    need(g("canary_not_ready")["detail"].get("error") == "script_canary_not_ready",
         "canary_not_ready: error が script_canary_not_ready でない")
if g("rollback"):
    ev = g("rollback")["events"]
    need(any(e.get("to_mode") == "rust" for e in ev), "rollback: rust への mode の変更の event が無い")
    need(any(e.get("to_state") == "retired" for e in ev), "rollback: generation の retired の event が無い")
if g("canary_denied_non_human"):
    need(not g("canary_denied_non_human")["actor"].startswith("human:"), "canary_denied_non_human: actor が人")
if g("canary"):
    c = g("canary")
    need(c["actor"].startswith("human:"), "canary: 人の token でない")
    need("human_approval_in_production" in c["detail"], "canary: 本番では人の承認に当たることの記録が無い")
    need(all(e["actor"] == c["actor"] for e in c["events"]), "canary: event の actor が人でない")
    need(any(e.get("to_mode") == "canary" for e in c["events"]), "canary: mode の変更の event が無い")
if g("behavior_change_denied_non_human"):
    d = g("behavior_change_denied_non_human")["detail"]
    need(d.get("http_status") == 403 and d.get("error") == "script_behavior_change_requires_human" and d.get("table_rows") == 0,
         "behavior_change_denied_non_human: 403・表は不変でない")
if g("behavior_change_approve"):
    c = g("behavior_change_approve")
    need(c["actor"].startswith("human:") and c["detail"].get("state") == "approved", "behavior_change_approve: 人の承認でない")
if g("guard_seed"):
    need(g("guard_seed")["detail"].get("synthetic") is True, "guard_seed: synthetic の印が無い")
if g("diff"):
    d = g("diff")["detail"]
    need(d.get("fixture_cases", 0) > 0 and d.get("fixture_diffs") == 0, "diff: fixture で差分 0 でない")
    need(d.get("probe_diffs", 0) > 0, "diff: 改善の当たる入力で差分が出ていない")
if g("restart_restore_canary"):
    d = g("restart_restore_canary")["detail"]
    need(d.get("mode") == "canary" and d.get("generation_state") == "active", "restart_restore_canary: 復元していない")
if g("rollback"):
    d = g("rollback")["detail"]
    need(d.get("mode") == "rust", "rollback: mode が rust でない")
if g("restart_restore_rust"):
    d = g("restart_restore_rust")["detail"]
    need(d.get("mode") == "rust" and d.get("generation_state") == "retired", "restart_restore_rust: rust・retired でない")

if errors:
    for e in errors:
        print("ts-pilot: " + e, file=sys.stderr)
    sys.exit(1)
print(f"ts-pilot: 記録 OK（{len(lines)} 段）")
PY
}

if [ "$cmd" = --validate ]; then
  validate "$arg"
  exit $?
fi

# ---- pilot の実行 ----
root="$(git rev-parse --show-toplevel)"
cd "$root"
mkdir -p "$arg"
out="$(cd "$arg" && pwd)"
record="$out/record.jsonl"
: >"$record"
commit="$(git rev-parse HEAD)"
: "${CARGO_TARGET_DIR:?ts-pilot: CARGO_TARGET_DIR が要る（celeris と script-deploy の binary を置く）}"

work="$(mktemp -d "${TMPDIR:-/tmp}/ts-pilot.XXXXXX")"
tree="$work/tree"
daemon_pid=""
cleanup() {
  if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [ -d "$tree" ]; then git -C "$root" worktree remove --force "$tree" >/dev/null 2>&1 || true; fi
  [ -n "${TS_PILOT_KEEP_WORK:-}" ] || rm -rf "$work"
}
trap cleanup EXIT

now() { date -u +%Y-%m-%dT%H:%M:%S.%6NZ; }
seq_no=0
# rec <stage> <started_at> <actor> <events json> <detail json>
rec() {
  seq_no=$((seq_no + 1))
  python3 -I - "$record" "$seq_no" "$1" "$2" "$(now)" "$3" "$4" "$5" <<'PY'
import json, sys
from datetime import datetime
path, seq, stage, t0, t1, actor, events, detail = sys.argv[1:9]
p = lambda v: datetime.fromisoformat(v.replace("Z", "+00:00"))
row = {"schema": "celeris.ts-pilot/1", "seq": int(seq), "stage": stage, "started_at": t0, "ended_at": t1,
       "secs": round((p(t1) - p(t0)).total_seconds(), 3), "actor": actor, "ok": True,
       "events": json.loads(events), "detail": json.loads(detail)}
with open(path, "a", encoding="utf-8") as f:
    f.write(json.dumps(row, ensure_ascii=False) + "\n")
print(f"ts-pilot: [{stage}] {row['secs']} 秒", file=sys.stderr)
PY
}
WORKER="worker:ts-pilot"

# 1. meta
t0="$(now)"
rec meta "$t0" "$WORKER" '[]' "$(python3 -I -c '
import json, socket, subprocess, sys
print(json.dumps({
  "kind": "policy_improvement",
  "note": "言語移行の測定（edit-activate-baseline.sh の policy・cos・policy-ts・cos-ts）とは別の記録。方針の改善を TS 制御コードに入れて段階昇格と rollback を通す",
  "commit": sys.argv[1],
  "host": socket.gethostname(),
  "node": subprocess.run(["node", "--version"], capture_output=True, text=True).stdout.strip(),
  "production_touched": False,
  "non_production": "daemon は $TMPDIR の一時 DB・一時 state dir・loopback の port。本番の mode・deploy・config・DB に触れない",
  "llm_usage": "この script は LLM を呼ばない。pilot を走らせた run の LLM 利用量は run の記録から取る（取れなければ未確認）",
}, ensure_ascii=False))' "$commit")"

# 2. edit: 使い捨ての作業ツリーで cos_digest の方針を改善する
t0="$(now)"
git worktree add --detach "$tree" HEAD >/dev/null 2>&1
if [ -n "${TS_PILOT_PATCH:-}" ]; then
  git -C "$tree" apply --check "$TS_PILOT_PATCH" || { echo "ts-pilot: 候補の patch が HEAD に当たらない" >&2; exit 1; }
  outside="$(git -C "$tree" apply --numstat "$TS_PILOT_PATCH" | awk '{print $3}' | grep -v '^control-ts/src/' || true)"
  if [ -n "$outside" ]; then
    echo "ts-pilot: 候補の patch が control-ts/src/ の外を変える: $outside" >&2
    exit 1
  fi
  git -C "$tree" apply "$TS_PILOT_PATCH"
  git -C "$tree" add -N . >/dev/null 2>&1
else
python3 -I - "$tree/control-ts/src/domains" <<'PY'
import sys, pathlib
d = pathlib.Path(sys.argv[1])
src = d / "cos_digest.ts"
s = src.read_text(encoding="utf-8")
old = '    if (reason !== null && rustTrim(reason) !== "") text += `: ${reason}`;\n'
new = ('    if (reason !== null && rustTrim(reason) !== "") text += `: ${capRunReason(reason)}`;\n')
assert s.count(old) == 1, "anchor (run の失敗の理由) が 1 か所でない"
s = s.replace(old, new)
anchor = '/** Rust の `cos_digest_message`: digest の文面と cards。 */\n'
assert s.count(anchor) == 1, "anchor (digestMessage) が 1 か所でない"
helper = (
    "/** ts-pilot の方針改善: 見出しに出す run の失敗の理由を 200 文字（code point）までにし、超えたら「…」で切る。\n"
    " * 例外の stack など長い理由が digest の見出しを埋めないようにする。cards の理由は変えない。 */\n"
    "export const RUN_REASON_MAX_CHARS = 200;\n"
    "export function capRunReason(reason: string): string {\n"
    "  const chars = Array.from(reason);\n"
    "  return chars.length <= RUN_REASON_MAX_CHARS ? reason : `${chars.slice(0, RUN_REASON_MAX_CHARS).join(\"\")}…`;\n"
    "}\n\n"
)
s = s.replace(anchor, helper + anchor)
src.write_text(s, encoding="utf-8")
t = d / "cos_digest.test.ts"
ts = t.read_text(encoding="utf-8")
imp = "  cosDigestPass,\n"
assert ts.count(imp) == 1
ts = ts.replace(imp, "  capRunReason,\n" + imp, 1)
ts += (
    "\ndescribe(\"capRunReason（ts-pilot の方針改善）\", () => {\n"
    "  it(\"200 文字までは変えず、超えたら 200 文字と「…」にする\", () => {\n"
    "    expect(capRunReason(\"x\".repeat(200))).toBe(\"x\".repeat(200));\n"
    "    expect(capRunReason(\"あ\".repeat(201))).toBe(`${\"あ\".repeat(200)}…`);\n"
    "    expect(capRunReason(\"😀\".repeat(201))).toBe(`${\"😀\".repeat(200)}…`);\n"
    "  });\n"
    "});\n"
)
t.write_text(ts, encoding="utf-8")
PY
fi
git -C "$tree" diff >"$out/candidate.patch"
files_json="$(git -C "$tree" diff --name-only | python3 -I -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')"
rec edit "$t0" "$WORKER" '[]' "$(python3 -I -c '
import hashlib, json, sys
p = open(sys.argv[1], "rb").read()
print(json.dumps({"domain": "cos_digest", "files": json.loads(sys.argv[2]),
  "change": sys.argv[3], "source": sys.argv[4],
  "patch": "candidate.patch", "patch_sha256": hashlib.sha256(p).hexdigest(),
  "patch_lines": p.count(b"\n"), "worktree": "$TMPDIR の git worktree --detach HEAD（この branch には commit しない）"},
  ensure_ascii=False))' "$out/candidate.patch" "$files_json" \
  "${TS_PILOT_CHANGE:-digest の見出しの run の失敗の理由を 200 文字（code point）までにし、超えたら「…」で切る}" \
  "$( [ -n "${TS_PILOT_PATCH:-}" ] && echo patch || echo builtin )")"

# 3. ts-verify（候補の作業ツリーで。基準は HEAD）
t0="$(now)"
set +e
(cd "$tree" && CELERIS_WU_BASE="$commit" bash scripts/dev/ts-verify.sh --record "$out/ts-verify.json") >"$work/ts-verify.log" 2>&1
tv_rc=$?
set -e
if [ "$tv_rc" -ne 0 ]; then
  tail -30 "$work/ts-verify.log" >&2
  echo "ts-pilot: ts-verify.sh が失敗した（exit $tv_rc）" >&2
  exit 1
fi
rec ts_verify "$t0" "$WORKER" '[]' "$(python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
print(json.dumps({"exit": int(sys.argv[2]), "class": r.get("class"), "record": "ts-verify.json",
  "stages": {s["name"]: s["secs"] for s in r.get("stages", [])},
  "total_secs": r.get("total_secs"), "cargo_invocations": r.get("cargo_invocations"), "units": r.get("units")}))' \
  "$out/ts-verify.json" "$tv_rc")"

# 4. 候補 bundle（ts-verify と同じ build-bundle.mjs。作業ツリーの control-ts/dist は使わない）
t0="$(now)"
cand="$work/candidate-dist"
(cd "$tree/control-ts" && TS_PILOT_BUILD="$tree/control-ts/scripts/build-bundle.mjs" TS_PILOT_OUT="$cand" \
  node --input-type=module -e '
    const { buildBundle } = await import(process.env.TS_PILOT_BUILD);
    await buildBundle({ outDir: process.env.TS_PILOT_OUT });
  ') >"$work/bundle.log" 2>&1
sha="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["bundle_sha256"])' "$cand/manifest.json")"
rec bundle "$t0" "$WORKER" '[]' "$(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1]))
print(json.dumps({k: m.get(k) for k in ["bundle_sha256", "host_api", "schema_sha256", "domains", "checkpoint_versions"]}))' "$cand/manifest.json")"

# 5. 試験用の daemon（一時 DB）
bin="$CARGO_TARGET_DIR/debug"
[ -x "$bin/celeris" ] && [ -x "$bin/script-deploy" ] || {
  echo "ts-pilot: celeris・script-deploy の binary を作る" >&2
  cargo build -p celeris --bin celeris -p celeris-script --bin script-deploy >"$work/build.log" 2>&1
}
home="$work/daemon"
mkdir -p "$home/ws"
printf 'pilot-human-token\n' >"$home/human.token"
printf 'pilot-operator-token\n' >"$home/operator.token"
port="$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cat >"$home/config.toml" <<EOF
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = ["-c", "cat >/dev/null; echo '{\"type\":\"question\",\"text\":\"not needed\"}'"]

[route_defaults.fake]
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]

[cos]
enabled = false

# 既定の build_cache_dir・scratch は本番の ~/.local/celeris の下なので、一時 dir に替える（本番の cache を掃除・参照しない）。
[workspace]
build_cache_dir = "$home/build-cache"

[scratch]
dir = "$home/scratch"

[api]
listen = "127.0.0.1:$port"

[[api.tokens]]
principal = "human:pilot"
token_file = "human.token"

[[api.tokens]]
principal = "operator:pilot"
token_file = "operator.token"

[script]
state_dir = "state"
EOF

start_daemon() {
  "$bin/celeris" --config "$home/config.toml" --log-format text --release pilot >>"$work/daemon.log" 2>&1 &
  daemon_pid=$!
  local i
  for i in $(seq 1 600); do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
      tail -30 "$work/daemon.log" >&2; echo "ts-pilot: daemon が終わった" >&2; return 1
    fi
    if curl -fs -o /dev/null -H "Authorization: Bearer pilot-human-token" "http://127.0.0.1:$port/api/v1/script/domains"; then
      return 0
    fi
    sleep 0.2
  done
  echo "ts-pilot: daemon の API が 120 秒で答えない" >&2
  return 1
}
stop_daemon() {
  kill -TERM "$daemon_pid"
  wait "$daemon_pid" 2>/dev/null || true
  daemon_pid=""
}

t0="$(now)"
start_daemon
rec daemon_start "$t0" "$WORKER" '[]' "$(python3 -I -c '
import json, sys
print(json.dumps({"db": "$TMPDIR/…/daemon/celeris.sqlite3（一時）", "state_dir": "$TMPDIR/…/daemon/state（一時）",
  "listen": "127.0.0.1:" + sys.argv[1], "release": "pilot", "cos": "無効", "llm_sources": "fake のみ",
  "tokens": {"human:pilot": "人の token（試験 daemon の中だけ）", "operator:pilot": "外部 operator の token（CoS 相当: 人でない主体）"}},
  ensure_ascii=False))' "$port")"

t0="$(now)"
deployed="$("$bin/script-deploy" --dist "$cand" --state-dir "$home/state")"
[ "$deployed" = "$sha" ] || { echo "ts-pilot: script-deploy の hash が違う: $deployed" >&2; exit 1; }
rec deploy "$t0" "$WORKER" '[]' "{\"bundle_sha256\": \"$deployed\", \"to\": \"<state_dir>/script-bundles/$deployed/\"}"

# 6. 以後の段は API と一時 DB の記録を読む driver が書く
TS_PILOT_SEQ="$seq_no" python3 -I - "$record" "$port" "$home" "$sha" <<'PY'
import json, os, sqlite3, sys, time, urllib.request, urllib.error
from datetime import datetime, timezone

record, port, home, sha = sys.argv[1:5]
seq = int(os.environ["TS_PILOT_SEQ"])
base = f"http://127.0.0.1:{port}/api/v1"
HUMAN, OPERATOR = "pilot-human-token", "pilot-operator-token"
db = os.path.join(home, "celeris.sqlite3")
DOMAIN = "cos_digest"

def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")

def rec(stage, t0, actor, events, detail):
    global seq
    seq += 1
    t1 = now()
    p = lambda v: datetime.fromisoformat(v.replace("Z", "+00:00"))
    row = {"schema": "celeris.ts-pilot/1", "seq": seq, "stage": stage, "started_at": t0, "ended_at": t1,
           "secs": round((p(t1) - p(t0)).total_seconds(), 3), "actor": actor, "ok": True,
           "events": events, "detail": detail}
    with open(record, "a", encoding="utf-8") as f:
        f.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"ts-pilot: [{stage}] {row['secs']} 秒", file=sys.stderr)

def api(method, path, token, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw.decode(errors="replace")

def conn():
    c = sqlite3.connect(db, timeout=30)
    c.execute("PRAGMA busy_timeout = 30000")
    return c

cursor = {"script_domain_events": 0, "script_generation_events": 0}
def new_events():
    out = []
    with conn() as c:
        for table in cursor:
            for id_, ts, j in c.execute(f"SELECT id, ts, json FROM {table} WHERE id > ? ORDER BY id", (cursor[table],)):
                cursor[table] = id_
                d = json.loads(j)
                out.append({"table": table, "id": id_, "ts": ts, "type": d.get("type"), "actor": d.get("actor"),
                            **{k: d[k] for k in ("domain", "from_mode", "to_mode", "from_generation", "to_generation",
                                                 "generation_id", "from_state", "to_state", "canary_bps", "version",
                                                 "reason", "cancelled_in_flight") if d.get(k) is not None}})
    return out

def domain():
    st, body = api("GET", "/script/domains", HUMAN)
    assert st == 200, (st, body)
    return next(d for d in body["domains"] if d["domain"] == DOMAIN)

def generation():
    st, body = api("GET", "/script/generations", HUMAN)
    assert st == 200, (st, body)
    return next(g for g in body["generations"] if g["bundle_sha256"] == sha)

def put_mode(token, mode, gen=None, bps=None, reason="ts-pilot"):
    body = {"mode": mode, "expected_version": domain()["version"], "reason": reason}
    if gen:
        body["generation"] = gen
    if bps is not None:
        body["canary_bps"] = bps
    return api("PUT", f"/script/domains/{DOMAIN}/mode", token, body)

def fail(msg):
    print("ts-pilot: " + msg, file=sys.stderr)
    sys.exit(1)

# 起動の前に一時 DB の記録は無い（cursor は 0 から読む）。
# register（operator の token。登録は挙動を変えないので人でない主体で行える。付記 S5-1）
t0 = now()
for _ in range(300):
    st, body = api("POST", "/script/generations", OPERATOR, {"bundle_sha256": sha})
    if st != 503:
        break
    time.sleep(0.2)
if st != 200 or body.get("state") != "loaded":
    fail(f"register: {st} {body}")
gen_id = body["id"]
rec("register", t0, "operator:pilot", new_events(),
    {"http_status": st, "bundle_sha256": sha, "generation_id": gen_id, "state": body["state"], "domains": body.get("domains")})

# compare（CoS 相当: 人でない operator の token。付記 S5-4 の 1 行目）
t0 = now()
st, body = put_mode(OPERATOR, "compare", sha)
if st != 200:
    fail(f"compare: {st} {body}")
d = domain()
rec("compare", t0, "operator:pilot", new_events(),
    {"http_status": st, "mode": d["mode"], "generation_state": d.get("generation_state"),
     "authority": "人でない主体（CoS operations・外部 operator と同じ権限の段）。compare は挙動を変えない"})

# 人でない主体は canary に上げられない（403、表は変わらない）
t0 = now()
before = domain()["version"]
st, body = put_mode(OPERATOR, "canary", sha, 5000)
ev = new_events()
if st != 403 or domain()["version"] != before or ev:
    fail(f"canary by operator: {st} {body} events={ev}")
rec("canary_denied_non_human", t0, "operator:pilot", ev,
    {"http_status": st, "error": body.get("code") if isinstance(body, dict) else body, "table_unchanged": True})

# 計数が (f) の進む条件に届く前は人の token でも 422
t0 = now()
st, body = put_mode(HUMAN, "canary", sha, 5000)
ev = new_events()
if st != 422 or ev:
    fail(f"canary before ready: {st} {body} events={ev}")
rec("canary_not_ready", t0, "human:pilot", ev, {"http_status": st, "error": body.get("code") if isinstance(body, dict) else body,
    "body": body})

# P2-G: 課題の改善仕様の承認（ADR 2026-11-03-ts-intended-behavior-change D4）。人でない主体は 403、人の token だけが通る。
spec_path = os.environ.get("TS_PILOT_SPEC", "")
if spec_path:
    spec = json.load(open(spec_path, encoding="utf-8"))
    st, body = api("POST", "/script/behavior-changes", HUMAN, {"spec": spec, "spec_sha256": "0" * 64, "reason": "hash の確認"})
    import re
    m = re.search(r"computed ([0-9a-f]{64})", body.get("detail", "") if isinstance(body, dict) else "")
    computed = m.group(1) if m else None
    if st != 422 or not computed:
        fail(f"behavior-change hash: {st} {body}")
    t0 = now()
    st, body = api("POST", "/script/behavior-changes", OPERATOR, {"spec": spec, "spec_sha256": computed, "reason": "P2-G"})
    with conn() as c:
        rows = c.execute("SELECT COUNT(*) FROM script_behavior_changes").fetchone()[0]
    code = body.get("code") if isinstance(body, dict) else body
    if st != 403 or rows != 0:
        fail(f"behavior-change by operator: {st} {body} rows={rows}")
    rec("behavior_change_denied_non_human", t0, "operator:pilot", [],
        {"http_status": st, "error": code, "table_rows": rows, "spec_id": spec["id"], "spec_sha256": computed})
    t0 = now()
    st, body = api("POST", "/script/behavior-changes", HUMAN, {"spec": spec, "spec_sha256": computed,
                                                              "reason": "P2-G: 試験 daemon の人の token"})
    with conn() as c:
        state = c.execute("SELECT state FROM script_behavior_changes WHERE spec_sha256 = ?", (computed,)).fetchone()
        evs = c.execute("SELECT action, actor FROM script_behavior_change_events WHERE spec_sha256 = ?", (computed,)).fetchall()
    if st != 200 or not state or state[0] != "approved":
        fail(f"behavior-change approve: {st} {body} state={state}")
    rec("behavior_change_approve", t0, "human:pilot",
        [{"table": "script_behavior_change_events", "action": a, "actor": who} for a, who in evs],
        {"http_status": st, "state": state[0], "spec_id": spec["id"], "spec_version": spec["version"], "spec_sha256": computed,
         "human_approval_in_production": "本番ではこの承認も人が行う（人の token だけ。D4）。P2-G は試験 daemon の人の token で行い、本番では承認していない"})

# 計数を積む（gen-e2e の observe_until_ready と同じ。一時 DB だけ）
t0 = now()
required = 200  # SCRIPT_GUARD_READY_COS（(f): CoS の経路は 200 件以上で説明のつかない不一致 0 件）
with conn() as c:
    c.execute(
        'INSERT INTO script_guard_state (domain, generation_id, mode, "window", consecutive_failures, timeouts_10min, '
        "clean_streak, ts_side_clean_streak, calls, safety_violations, updated_at) "
        "VALUES (?, ?, 'compare', ?, 0, '[]', ?, 0, ?, 0, ?) "
        'ON CONFLICT (domain, generation_id) DO UPDATE SET mode = excluded.mode, "window" = excluded."window", '
        "clean_streak = excluded.clean_streak, calls = excluded.calls, updated_at = excluded.updated_at",
        (DOMAIN, gen_id, "o" * required, required, required, now()))
rec("guard_seed", t0, "worker:ts-pilot", [],
    {"synthetic": True, "table": "script_guard_state（一時 DB）", "calls_ok": required, "required": required,
     "why": "試験 daemon には CoS の実 pass が無い。gen-e2e の observe_until_ready と同じく成功の呼び出しの計数だけを積んだ",
     "production": "本番では compare の実 pass 200 件（説明のつかない不一致 0・timeout と失敗の率 0.1% 以下・安全違反 0）が要る。計数を書き換えない"})

# canary（試験 daemon の人の token）
t0 = now()
st, body = put_mode(HUMAN, "canary", sha, 5000, reason="ts-pilot: canary（試験 daemon の人の token）")
if st != 200:
    fail(f"canary: {st} {body}")
d = domain()
rec("canary", t0, "human:pilot", new_events(),
    {"http_status": st, "mode": d["mode"], "canary_bps": d.get("canary_bps"), "generation_state": d.get("generation_state"),
     "human_approval_in_production": "本番ではこの段が人の承認に当たる（compare→canary・canary→ts は人の token だけ。付記 S5-4）。pilot は試験 daemon の人の token で行い、本番の mode は変えていない"})
print(json.dumps({"generation_id": gen_id}), file=open(os.path.join(home, "pilot-state.json"), "w"))
PY
seq_no="$(wc -l <"$record")"

# 7. 差分の記録: 改善前（HEAD）と候補の decide を fixture と改善の当たる入力で比べる
t0="$(now)"
mkdir -p "$work/diff"
git -C "$root" show "HEAD:control-ts/src/domains/cos_digest.ts" >"$work/diff/base.ts"
cp "$tree/control-ts/src/domains/cos_digest.ts" "$work/diff/cand.ts"
cat >"$work/diff/build.mjs" <<'JS'
import { createRequire } from "node:module";
const [esbuildPath, input, outfile] = process.argv.slice(2);
const esbuild = createRequire(esbuildPath)("esbuild");
const stub = { name: "host-stub", setup(b) {
  b.onResolve({ filter: /^celeris:host$/ }, () => ({ path: "host", namespace: "stub" }));
  b.onLoad({ filter: /.*/, namespace: "stub" }, () => ({ contents: "export const cosDigestListRuns = () => { throw new Error('stub'); }; export const cosDigestReadRun = cosDigestListRuns;", loader: "js" }));
} };
await esbuild.build({ entryPoints: [input], outfile, bundle: true, format: "esm", platform: "node", plugins: [stub], logLevel: "error" });
JS
esb="$tree/control-ts/node_modules/esbuild/package.json"
node "$work/diff/build.mjs" "$esb" "$work/diff/base.ts" "$work/diff/base.mjs"
node "$work/diff/build.mjs" "$esb" "$work/diff/cand.ts" "$work/diff/cand.mjs"
cat >"$work/diff/compare.mjs" <<'JS'
import { readFileSync } from "node:fs";
const [baseMod, candMod, cases, traces, specCases] = process.argv.slice(2);
const base = await import(baseMod), cand = await import(candMod);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
let fixtureCases = 0, fixtureDiffs = 0;
const failed = [];
for (const line of readFileSync(cases, "utf8").split("\n")) {
  if (!line.trim()) continue;
  const c = JSON.parse(line);
  fixtureCases++;
  let a, b;
  try { a = base.decide(c.input); } catch (e) { a = { error: String(e.message) }; }
  try { b = cand.decide(c.input); } catch (e) { b = { error: String(e.message) }; }
  if (!same(a, b)) fixtureDiffs++;
  if (c.input.run_state === "failed" && failed.length < 3) failed.push(c.input);
}
const traceLines = readFileSync(traces, "utf8").split("\n").filter((l) => l.trim()).length;
// 改善の当たる入力: fixture の failed の入力の reason だけを長さ 150・200・201・500・4000 に替える。
const lens = specCases ? null : [150, 200, 201, 500, 4000];
const probes = [];
// P2-G: 課題の期待差分 fixture があれば、その入力を probe にする。
if (specCases) for (const l of readFileSync(specCases, "utf8").split("\n")) { if (l.trim()) probes.push(JSON.parse(l).input); }
else for (const input of failed) for (const n of lens) probes.push({ ...input, reason: "E".repeat(n) });
let probeDiffs = 0;
const examples = [];
for (const p of probes) {
  const a = base.decide(p), b = cand.decide(p);
  if (!same(a, b)) {
    probeDiffs++;
    if (examples.length < 2 && !specCases) {
      const head = (o) => JSON.stringify(o).match(/run は失敗: (E+)(…?)/);
      const ha = head(a), hb = head(b);
      examples.push({ reason_chars: p.reason.length, base_reason_in_heading: ha ? ha[1].length : null,
        candidate_reason_in_heading: hb ? hb[1].length : null, candidate_ellipsis: hb ? hb[2] === "…" : null });
    }
  }
}
console.log(JSON.stringify({ fixture_cases: fixtureCases, fixture_diffs: fixtureDiffs, pass_trace_lines: traceLines,
  probe_cases: probes.length, probe_diffs: probeDiffs, probe_lengths: lens, examples,
  probe_source: specCases ? "課題の期待差分 fixture の入力" : "fixture の failed の入力の reason の長さを替えたもの",
  expected: specCases ? "fixture の差分 0。期待差分 fixture の explained の入力だけが違う（一致は ts-behavior-eval が数える）"
    : "fixture の差分 0（ts-verify の golden と同じ）。reason が 201 文字以上の probe だけが違う" }));
JS
diff_json="$(node "$work/diff/compare.mjs" "$work/diff/base.mjs" "$work/diff/cand.mjs" \
  "$tree/control-ts/fixtures/cos_digest/cases.jsonl" "$tree/control-ts/fixtures/cos_digest/pass_traces.jsonl" \
  ${TS_PILOT_SPEC:+"${TS_PILOT_SPEC%.json}.cases.jsonl"})"
rec diff "$t0" "$WORKER" '[]' "$(python3 -I -c '
import json, sys
d = json.loads(sys.argv[1])
d["compared"] = "改善前（HEAD の cos_digest.ts。Rust の参照実装と golden で一致）と候補の decide"
d["note"] = "候補は Rust の参照実装と長い理由の入力で違う。compare・canary の間にその入力が来ると script-guard は説明のつかない不一致として rust へ戻す（S5 は正規化の規則を持たない）。方針改善を TS で出すには、二重実装の間は Rust の参照実装にも同じ変更が要る"
print(json.dumps(d, ensure_ascii=False))' "$diff_json")"

# 8. 再起動 → canary と generation の復元 → rollback → 再起動 → rust の復元
# 1 回目の停止はこの shell が行う（daemon はこの shell の子なので、wait で回収する）。
t_restart="$(now)"
stop_daemon
TS_PILOT_SEQ="$seq_no" TS_PILOT_BIN="$bin/celeris" TS_PILOT_LOG="$work/daemon.log" TS_PILOT_T0="$t_restart" \
  python3 -I - "$record" "$port" "$home" "$sha" <<'PY'
import json, os, signal, sqlite3, subprocess, sys, time, urllib.request, urllib.error
from datetime import datetime, timezone

record, port, home, sha = sys.argv[1:5]
seq = int(os.environ["TS_PILOT_SEQ"])
base = f"http://127.0.0.1:{port}/api/v1"
HUMAN, OPERATOR = "pilot-human-token", "pilot-operator-token"
db = os.path.join(home, "celeris.sqlite3")
DOMAIN = "cos_digest"
pid = 0
log = open(os.environ["TS_PILOT_LOG"], "ab")
proc = None

def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")

def rec(stage, t0, actor, events, detail):
    global seq
    seq += 1
    t1 = now()
    p = lambda v: datetime.fromisoformat(v.replace("Z", "+00:00"))
    row = {"schema": "celeris.ts-pilot/1", "seq": seq, "stage": stage, "started_at": t0, "ended_at": t1,
           "secs": round((p(t1) - p(t0)).total_seconds(), 3), "actor": actor, "ok": True,
           "events": events, "detail": detail}
    with open(record, "a", encoding="utf-8") as f:
        f.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"ts-pilot: [{stage}] {row['secs']} 秒", file=sys.stderr)

def api(method, path, token, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw.decode(errors="replace")
    except (urllib.error.URLError, ConnectionError, OSError):
        return 0, None

def fail(msg):
    print("ts-pilot: " + msg, file=sys.stderr)
    stop()
    sys.exit(1)

def max_ids():
    c = sqlite3.connect(db, timeout=30)
    try:
        return {t: c.execute(f"SELECT COALESCE(MAX(id), 0) FROM {t}").fetchone()[0]
                for t in ("script_domain_events", "script_generation_events")}
    finally:
        c.close()

cursor = max_ids()
def new_events():
    out = []
    c = sqlite3.connect(db, timeout=30)
    try:
        for table in cursor:
            for id_, ts, j in c.execute(f"SELECT id, ts, json FROM {table} WHERE id > ? ORDER BY id", (cursor[table],)):
                cursor[table] = id_
                d = json.loads(j)
                out.append({"table": table, "id": id_, "ts": ts, "type": d.get("type"), "actor": d.get("actor"),
                            **{k: d[k] for k in ("domain", "from_mode", "to_mode", "from_generation", "to_generation",
                                                 "generation_id", "from_state", "to_state", "canary_bps", "version",
                                                 "reason", "cancelled_in_flight") if d.get(k) is not None}})
    finally:
        c.close()
    return out

def domain():
    st, body = api("GET", "/script/domains", HUMAN)
    assert st == 200, (st, body)
    return next(d for d in body["domains"] if d["domain"] == DOMAIN)

def generation():
    st, body = api("GET", "/script/generations", HUMAN)
    assert st == 200, (st, body)
    return next(g for g in body["generations"] if g["bundle_sha256"] == sha)

def stop():
    global pid, proc
    if pid:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        proc.wait(timeout=120)
        pid = 0

def start():
    global pid, proc
    proc = subprocess.Popen([os.environ["TS_PILOT_BIN"], "--config", os.path.join(home, "config.toml"),
                             "--log-format", "text", "--release", "pilot"], stdout=log, stderr=log)
    pid = proc.pid
    for _ in range(600):
        if proc.poll() is not None:
            fail(f"daemon が終わった（exit {proc.returncode}）")
        st, _ = api("GET", "/script/domains", HUMAN)
        if st == 200:
            return
        time.sleep(0.2)
    fail("daemon の API が 120 秒で答えない")

def restart():
    stop()
    start()
    # 起動時の復元（S5-2）は API の前に終わる。generation の行が読めるまで待つ。
    for _ in range(300):
        st, body = api("GET", "/script/generations", HUMAN)
        if st == 200:
            return
        time.sleep(0.2)
    fail("generation の一覧が読めない")

# 再起動 1: canary と active の generation を復元する（停止は shell が済ませた。t0 は停止の前）
t0 = os.environ["TS_PILOT_T0"]
start()
d, g = domain(), generation()
ev = new_events()
if d["mode"] != "canary" or d.get("bundle_sha256") != sha or g["state"] != "active":
    fail(f"restart (canary): {d} {g}")
rec("restart_restore_canary", t0, "worker:ts-pilot", ev,
    {"mode": d["mode"], "canary_bps": d.get("canary_bps"), "bundle_sha256": d.get("bundle_sha256"),
     "generation_state": g["state"], "version": d["version"],
     "restored_by": "daemon の起動時の復元（付記 S5-2）。表を変えないので event は書かない（events が空なのは正しい）",
     "check": "daemon を SIGTERM で止めて同じ一時 DB で起こし、mode canary と generation（active）を新しい isolate で load し直した"})

# rollback: rust へ戻す（戻しは認証された誰でも。CoS 相当の operator の token で行う。付記 S5-4）
t0 = now()
st, body = api("PUT", f"/script/domains/{DOMAIN}/mode", OPERATOR,
               {"mode": "rust", "expected_version": domain()["version"], "reason": "ts-pilot: rollback"})
if st != 200:
    fail(f"rollback: {st} {body}")
state = None
for _ in range(600):  # draining → retired（in-flight 0 で見回りが捨てる。打ち切りの上限は 30 秒）
    state = generation()["state"]
    if state == "retired":
        break
    time.sleep(0.2)
d = domain()
ev = new_events()
if d["mode"] != "rust" or state != "retired":
    fail(f"rollback: mode={d['mode']} generation={state}")
rec("rollback", t0, "operator:pilot", ev,
    {"http_status": st, "mode": d["mode"], "generation_state": state,
     "known_good": "Rust の参照実装（mode rust）。候補の bundle の file と retired の行は残る（再登録で compare からやり直せる）"})

# 再起動 2: rust のまま起動し、retired の generation を load しない
t0 = now()
restart()
d, g = domain(), generation()
ev = new_events()
if d["mode"] != "rust" or g["state"] != "retired":
    fail(f"restart (rust): {d} {g}")
rec("restart_restore_rust", t0, "worker:ts-pilot", ev,
    {"mode": d["mode"], "generation_id": d.get("generation_id"), "generation_state": g["state"], "version": d["version"],
     "restored_by": "daemon の起動時の復元（付記 S5-2）。表を変えないので event は書かない",
     "check": "rollback の後に再起動しても mode rust のまま。retired の generation は load しない"})
stop()
PY
daemon_pid=""

validate "$record"
echo "ts-pilot: 完了。記録 $record" >&2
