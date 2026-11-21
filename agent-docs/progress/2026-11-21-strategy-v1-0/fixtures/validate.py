#!/usr/bin/env python3
"""議論・引用の共有フォーマット（discussion envelope）の参照の検査器（仕様の oracle。本番の code ではない）。

  python3 -I <dir>/validate.py verify     # baseline は受理、否定対照は期待の理由で拒否されること
  python3 -I <dir>/validate.py ablate     # 規則を 1 つずつ外すと verify が落ちること（各規則を否定対照が守っている）
子 2〜4 はこの fixture と規則（R1〜R13）を Rust・TS の試験に写す。規則の意味は design-brief.md の「共有フォーマット」。
"""
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MAX_BODY = 16384  # 入力長の上限（byte、UTF-8）
SOURCES = {"celeris_cos_thread", "human_paste", "mcp_client", "markdown_import"}
EXTERNAL = {"human_paste", "mcp_client", "markdown_import"}
ACTOR_FOR = {"celeris_cos_thread": {"human", "cos"}, "human_paste": {"human"}, "mcp_client": {"mcp"}, "markdown_import": {"human"}}
STATES = {"discussion", "proposed", "approved", "rejected", "withdrawn"}
CONF = {"confirmed", "estimated", "contradicted", "needs_decision"}
SECRET = re.compile(r"(?i)(bearer\s+[a-z0-9._-]{16,}|password\s*[:=]|cookie\s*[:=]|set-cookie|https://hooks\.|sk-[a-z0-9]{20,}|ghp_[a-z0-9]{20,}|xox[bp]-)")
INJECT = re.compile(r"(?i)(ignore (all |the )?previous|you are now|承認したことに|以前の指示を無視|approve (this|the) (decision|plan)|publish (it|this) now)")


def h(body):
    return "sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest()


def rules(env, ctx):
    """違反の理由の list を返す。ctx は thread の project・既知の idempotency key・最新の revision。"""
    out = []
    src, actor, st = env.get("source", {}).get("kind"), env.get("actor", {}).get("kind"), env.get("decision_state")
    body = env.get("body", "")
    out += ["R1:required"] if any(k not in env for k in ("project_id", "source", "actor", "source_revision", "timestamp", "content_hash", "consent", "idempotency_key", "decision_state", "confidence", "references", "body")) else []
    out += ["R2:source_kind"] if src not in SOURCES else []
    out += ["R3:actor_mismatch"] if actor not in ACTOR_FOR.get(src, set()) else []
    out += ["R4:content_hash"] if env.get("content_hash") != h(body) else []
    out += ["R5:body_too_long"] if len(body.encode("utf-8")) > MAX_BODY else []
    out += ["R6:project_mismatch"] if env.get("project_id") != ctx["project_id"] else []
    seen = ctx["idempotency"].get(env.get("idempotency_key"))
    out += ["R7:idempotency_conflict"] if seen is not None and seen != env.get("content_hash") else []
    c = env.get("consent", {})
    out += ["R8:consent"] if src in EXTERNAL and not (c.get("granted_by", "").startswith("human:") and c.get("scope") in {"selected_excerpt", "summary"}) else []
    out += ["R9:secret"] if SECRET.search(body) else []
    appr = env.get("approval")
    out += ["R10:approval_needs_human"] if st == "approved" and not (appr and appr.get("by", "").startswith("human:") and appr.get("via") in {"web", "api_human_token"}) else []
    out += ["R11:external_cannot_decide"] if src in EXTERNAL and st not in {"discussion", "proposed"} else []
    out += ["R12:unsupported_claim"] if env.get("confidence") == "confirmed" and not env.get("references") else []
    out += ["R13:outbound_confidential"] if env.get("direction") == "outbound" and env.get("classification") == "confidential" and not (appr and appr.get("by", "").startswith("human:")) else []
    out += ["R2:source_kind"] if st not in STATES or env.get("confidence") not in CONF else []
    if INJECT.search(body) and (st not in {"discussion", "proposed"}):
        out.append("R11:external_cannot_decide")
    return sorted(set(out))


def load():
    ctx = json.loads((HERE / "context.json").read_text(encoding="utf-8"))
    cases = []
    for p in sorted(HERE.glob("*/*.json")):
        cases.append((p, json.loads(p.read_text(encoding="utf-8"))))
    return ctx, cases


def run(disabled=frozenset(), quiet=False):
    ctx, cases = load()
    ok = True
    for p, c in cases:
        got = [r for r in rules(c["envelope"], ctx) if r.split(":")[0] not in disabled]
        want = c["expect"]
        good = (want == "accept" and not got) or (want != "accept" and want in got)
        ok &= good
        if not quiet:
            print(f"{'ok  ' if good else 'FAIL'} {p.parent.name}/{p.name}: expect {want}, got {got or 'accept'}")
    return ok, len(cases)


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "verify"
    if cmd == "verify":
        ok, n = run()
        print(f"fixtures verify: {n} cases, {'ok' if ok else 'FAILED'}")
        return 0 if ok else 1
    if cmd == "ablate":
        bad = []
        for i in range(1, 14):
            ok, _ = run(disabled=frozenset({f"R{i}"}), quiet=True)
            print(f"{'ok  ' if not ok else 'FAIL'} R{i} を外すと verify は {'落ちる' if not ok else '通る（否定対照が無い）'}")
            if ok:
                bad.append(i)
        print("fixtures ablate:", "ok" if not bad else f"FAILED {bad}")
        return 0 if not bad else 1
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
