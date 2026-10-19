#!/usr/bin/env python3
"""副比較セット（pinned/sub/, 測定 ID mlsys-sub-1, B1/C1）の固定値を内容で検査する。LLM・daemon・network に触れない。

使い方: python3 scripts/campaign/check-sub-pins.py --root <artifacts dir>
不足・不一致は 1 件ずつ標準エラーへ出して exit 1。全部通れば exit 0。主比較 pinned/main/ は読むだけ。
Jev の段階別の実検査（鍵登録・分類呼び出し）はここでは行わない（別 task）。
"""
import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

SONNET, OPUS = "claude-sonnet-5-5", "claude-opus-5-5"
ARM_COMMON_KEYS = ("harness", "harness_version", "model", "model_version")


def sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True, type=Path)
    root = ap.parse_args().root
    errs: list[str] = []

    def bad(msg: str) -> None:
        errs.append(msg)

    sub, main_dir = root / "pinned" / "sub", root / "pinned" / "main"
    names = ["final.json", "daemon.toml", "launcher.env", "canaries.json", "task-template.json",
             "final-input-manifest.json", "env-lock.rehearsal.json"]
    for n in names:
        if not (sub / n).is_file():
            bad(f"missing file: pinned/sub/{n}")
        if not (main_dir / n).is_file():
            bad(f"missing file: pinned/main/{n}")
    if errs:
        return finish(errs)

    final = json.loads((sub / "final.json").read_text())
    mfinal = json.loads((main_dir / "final.json").read_text())
    arms = {c["id"]: c for c in final["configs"]}
    marms = {c["id"]: c for c in mfinal["configs"]}

    # --- manifest: 共通候補・階層・方策差 ---
    if final.get("campaign_id") != "mlsys-sub-1":
        bad("final.json campaign_id must be mlsys-sub-1")
    if final["campaign_id"] == mfinal["campaign_id"]:
        bad("sub campaign_id equals main (measurement ids must differ)")
    for k in ("budget", "termination", "snapshot", "repeats", "problems", "input", "primary", "secondary"):
        if final.get(k) != mfinal.get(k):
            bad(f"final.json {k} differs from main (budget/initial info must be common)")
    b, c = arms.get("B"), arms.get("C")
    if not b or not c:
        bad("final.json lacks arm B or C")
        return finish(errs)
    for k in ARM_COMMON_KEYS:
        if b.get(k) != c.get(k):
            bad(f"B and C differ in {k}")
        if b.get(k) != marms["B"].get(k):
            bad(f"sub B {k} differs from main B (release/hash pin)")
    br, cr = b.get("routing", {}), c.get("routing", {})
    if set(br.get("models", [])) != {SONNET, OPUS} or len(br.get("models", [])) != 2:
        bad(f"B routing.models must be exactly [{SONNET}, {OPUS}]")
    if br.get("models") != cr.get("models"):
        bad("B and C routing.models differ")
    if br.get("run_harnesses") != ["claude-code"] or cr.get("run_harnesses") != ["claude-code"]:
        bad("B/C routing.run_harnesses must be [claude-code]")
    for k in ("lower_policy", "online_update"):
        if br.get(k) != cr.get(k):
            bad(f"B and C differ in routing.{k}")
    if b.get("policy_version") != "fixed" or c.get("policy_version") != "jev-1":
        bad("policy_version must be B=fixed, C=jev-1")
    if any(k in br for k in ("evidence_table_id", "evidence_table")) or br.get("no_evidence_table"):
        bad("B must have no evidence table / no latest fallback field")
    if cr.get("no_evidence_table") is not True or "evidence_table_id" in cr:
        bad("C must set no_evidence_table=true and no evidence_table_id")
    if {k for k in br if k != "no_evidence_table"} != {k for k in cr if k != "no_evidence_table"}:
        bad("B and C routing keys differ beyond no_evidence_table")
    if arms["A"] != marms["A"]:
        bad("arm A differs from main")

    # --- daemon.toml: 候補・階層・harness・Jev ---
    d = tomllib.loads((sub / "daemon.toml").read_text())
    md = tomllib.loads((main_dir / "daemon.toml").read_text())
    src = d.get("llm_sources", {}).get("claude_oauth", {})
    if set(src.get("models", [])) != {SONNET, OPUS}:
        bad("daemon llm_sources.claude_oauth.models must equal routing.models")
    cc = d.get("route_defaults", {}).get("claude-code", {})
    cand = [x["model"] for x in cc.get("candidates", [])]
    if set(cand) != {SONNET, OPUS} or cand[:1] != [SONNET]:
        bad("route_defaults.claude-code candidates must be [sonnet, opus] (standard lane = Sonnet)")
    fr = [x["model"] for x in cc.get("lanes", {}).get("frontier", {}).get("candidates", [])]
    if fr[:1] != [OPUS] or set(fr) != {SONNET, OPUS}:
        bad("route_defaults.claude-code.lanes.frontier must be [opus, sonnet]")
    if set(cc.get("lanes", {})) != {"frontier"}:
        bad("only the frontier lane override is allowed (no single-lane pin)")
    for h in set(d.get("harnesses", {})) | set(d.get("route_defaults", {})):
        if h not in ("claude-code", "cap-probe-claude-code"):
            bad(f"unexpected harness {h} (run_harnesses is claude-code only)")
    cp = d.get("harnesses", {}).get("cap-probe-claude-code", {})
    if cp.get("kind") != "fake" or cp.get("env_of") != "claude-code":
        bad("cap-probe-claude-code must be kind=fake env_of=claude-code")
    if "cap-probe" not in d.get("llm_sources", {}) or "cap-probe-claude-code" not in d.get("route_defaults", {}):
        bad("cap-probe llm_source / route_defaults declaration missing")
    rules = d.get("harness_selection", {}).get("rules", [])
    if not rules or rules[0].get("genre") != "cap-probe-claude-code":
        bad("harness_selection.rules must start with the cap-probe-claude-code rule")
    if d.get("routing", {}).get("mode") != "fixed":
        bad("daemon [routing] mode must be fixed (C1 is pinned per task via routing_override)")
    jev = d.get("jev", {})
    if jev.get("enabled") is not True:
        bad("daemon [jev] enabled must be true (C1)")
    if jev.get("secret_id", "jev_api_key") != "jev_api_key":
        bad("[jev] secret_id must be the existing name jev_api_key")
    if set(jev) - {"enabled", "secret_id"}:
        bad("[jev] must not set anything but enabled/secret_id")
    if "jev" in d.get("llm_sources", {}):
        bad("[llm_sources.jev] must not exist (Jev is not a route candidate)")
    if "dir" not in d.get("secrets", {}):
        bad("[secrets] dir missing (where jev_api_key is read from)")
    # B1 へ Jev が漏れない根拠: 実装が task 単位で mode を解決する（試験で固定されている）ことを main の config と対で見る
    if "jev" in md:
        bad("main daemon.toml must not enable Jev")
    for k in ("db", "workspace_root"):
        if d.get(k) == md.get(k):
            bad(f"sub daemon {k} equals main")
    if d["api"]["listen"] == md["api"]["listen"]:
        bad("sub daemon api.listen equals main")
    text = (sub / "daemon.toml").read_text()
    for pat in (r"sk-[A-Za-z0-9_-]{16,}", r"Bearer\s+[A-Za-z0-9._-]{16,}", r"eyJ[A-Za-z0-9_-]{20,}",
                r"(?i)(api[_-]?key|token|password)\s*=\s*\"[^\"]{12,}\""):
        if re.search(pat, text):
            bad(f"daemon.toml looks like it contains a secret value ({pat})")

    # --- launcher.env: 実在 path・混在なし ---
    env = parse_env(sub / "launcher.env")
    menv = parse_env(main_dir / "launcher.env")
    if "/wu/" in (sub / "launcher.env").read_text():
        bad("launcher.env references a disappearing /wu/ work-unit path")
    deploy_only = {"DAEMON_CONFIG", "OUT", "STATE_DIR"}  # operator が配置・作成する
    for k, v in env.items():
        if v.startswith("/") and k not in deploy_only and k != "RUN_PATH" and not Path(v).exists():
            bad(f"launcher.env {k} does not exist: {v}")
        if "<" in v and ">" in v:
            bad(f"launcher.env {k} has a placeholder")
    for k in ("RELEASE", "CELERIS_BIN", "CAP_ENV", "A_ACCOUNT", "CAMPAIGN_BIN", "NORMAL_CONFIG",
              "MATERIALS_SRC", "KB_SRC", "REPO_SRC", "INPUTS_SRC"):
        if env.get(k) != menv.get(k):
            bad(f"launcher.env {k} differs from main (common pin)")
    if env.get("MEASUREMENT_ID") != "mlsys-sub-1":
        bad("launcher.env MEASUREMENT_ID must be mlsys-sub-1")
    for k in ("OUT", "STATE_DIR", "DAEMON_CONFIG"):
        if env.get(k) == menv.get(k):
            bad(f"launcher.env {k} equals main")
    if env.get("RELEASE") != b["harness_version"]:
        bad("launcher.env RELEASE != manifest harness_version")
    if cp.get("command") and not Path(cp["command"]).exists():
        bad(f"cap-probe command does not exist: {cp['command']}")
    if cp.get("command") != env.get("CAMPAIGN_BIN"):
        bad("cap-probe command must be the same binary as CAMPAIGN_BIN")
    for f in ("final-input-manifest.json", "canaries.json", "task-template.json"):
        if sha256(sub / f) != sha256(main_dir / f):
            bad(f"pinned/sub/{f} differs from main")

    # --- env-lock/3 と campaign sha ---
    lock = json.loads((sub / "env-lock.rehearsal.json").read_text())
    mlock = json.loads((main_dir / "env-lock.rehearsal.json").read_text())
    if lock.get("schema") != "celeris-env-lock/3":
        bad("env-lock schema must be celeris-env-lock/3")
    want = None
    try:
        r = subprocess.run([env["CAMPAIGN_BIN"], "validate", "--manifest", str(sub / "final.json")],
                           capture_output=True, text=True, timeout=60)
        if r.returncode != 0:
            bad(f"celeris-campaign validate failed: {r.stderr.strip()[:200]}")
        else:
            want = r.stdout.strip().splitlines()[-1]
    except OSError as e:
        bad(f"cannot run celeris-campaign validate: {e}")
    if want and lock.get("campaign_sha256") != want:
        bad(f"env-lock campaign_sha256 {lock.get('campaign_sha256')} != recomputed {want} (regenerate env-lock)")
    if lock.get("campaign_id") != "mlsys-sub-1" or lock["campaign_sha256"] == mlock["campaign_sha256"]:
        bad("env-lock must belong to mlsys-sub-1 (distinct hash from main)")
    if lock["bc"]["release"] != env.get("RELEASE"):
        bad("env-lock release != RELEASE")
    if lock.get("cap_env") != mlock.get("cap_env") or lock.get("a") != mlock.get("a"):
        bad("env-lock A program / cap_env differ from main (common pin)")
    larms = {a["id"]: a for a in lock["bc"]["arms"]}
    for aid, arm in (("B", b), ("C", c)):
        la = larms.get(aid)
        if not la:
            bad(f"env-lock lacks arm {aid}")
            continue
        r = arm["routing"]
        if la["models"] != r["models"] or la["run_harnesses"] != r["run_harnesses"]:
            bad(f"env-lock arm {aid} models/run_harnesses != manifest")
        if la["no_evidence_table"] != (r.get("no_evidence_table") is True) or la["evidence_table_id"] is not None:
            bad(f"env-lock arm {aid} evidence fields != manifest")
        ids = {x["model_id"] for x in la["catalog"]}
        if not {SONNET, OPUS} <= ids:
            bad(f"env-lock arm {aid} catalog lacks sonnet/opus")
    if larms.get("B", {}).get("catalog") != larms.get("C", {}).get("catalog"):
        bad("env-lock B/C catalog (candidates/tiers) differ")
    # campaign lock（lock.json）は preflight ok 後に watch の初回が作る。ここでは「あれば照合」だけ。
    lj = sub / "lock.json"
    if lj.exists():
        try:
            if json.loads(lj.read_text()).get("campaign_id") not in (None, "mlsys-sub-1"):
                bad("lock.json campaign_id is not mlsys-sub-1")
        except ValueError as e:
            bad(f"lock.json unreadable: {e}")

    # --- 固定 hash 一覧（報告書の表と実体） ---
    rep = (root / "measure-pinned-sub-report.md").read_text()
    for n in ("final.json", "daemon.toml", "launcher.env", "canaries.json", "task-template.json",
              "env-lock.rehearsal.json"):
        m = re.search(rf"\|\s*`?(?:pinned/sub/)?{re.escape(n)}`?\s*\|\s*([0-9a-f]{{64}})\s*\|", rep)
        if not m:
            bad(f"report hash table lacks a row for {n}")
        elif m.group(1) != sha256(sub / n):
            bad(f"report hash for {n} is stale: {m.group(1)} != {sha256(sub / n)}")
    m = re.search(r"campaign sha256（mlsys-sub-1）\s*\|\s*([0-9a-f]{64})", rep)
    if not m or (want and m.group(1) != want):
        bad("report campaign sha256 (mlsys-sub-1) is missing or stale")
    return finish(errs)


def parse_env(p: Path) -> dict:
    out = {}
    for line in p.read_text().splitlines():
        if line.strip() and not line.lstrip().startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            out[k.strip()] = v.strip()
    return out


def finish(errs: list[str]) -> int:
    if errs:
        for e in errs:
            print(f"FAIL: {e}", file=sys.stderr)
        print(f"check-sub-pins: {len(errs)} problem(s)", file=sys.stderr)
        return 1
    print("check-sub-pins: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
