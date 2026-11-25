#!/usr/bin/env python3
"""盲検の評価の出力を比べる（scripts/dev/parent-eval-blind.sh の後に人が走らせる）。

  python3 -I scripts/dev/parent-eval-blind-compare.py --out <dir> --answer-key <answer key の json>

<dir>/<case>/<point>/<arm>/output.json（verdict・next と、方式 b・c は最終の記録 record）・reads.json・run.json を読み、
時点・方式ごとに記録の status・verdict・next と、answer key の期待との一致を出す（方式 a は評価器を起こさないので status を見ない）。reads に作業 tree・一時 home の外の path、または禁じた名前（評価器に見せない文書の印）があれば
不合格。結果を <dir>/compare.json に書く（同じ入力で同じ byte。時刻を書かない）。
exit: 0 = 方式 b・c の全評価が checked で禁じた読みが無い（不一致でも 0）、1 = 記録が無い・checked でない・禁じた読み、2 = 使い方の誤り。

対照 pilot の指標（ADR 2026-11-24-strategy-pilot-ablation D4・D6）:

  python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs <dir> [--runs <dir> ...] --answer-key <answer_key.json> --out <dir>
      [--research-src <c-results.md>]

--runs は driver の出力（<dir>/<case>/<point>/<arm>/ と <dir>/<case>/strategy-v1/）。反復ごとの dir または反復を含む親 dir を渡す。dir の名を反復の名にする。
方式ごとに verdict・next と answer key の一致（replay）、誤った pivot と見逃し、根拠の参照が slice の外か無い claim の数、
未達発見までの task 数、検知の後の worker run と USD（replay の events.jsonl から）、usage（run 数・USD）、secret の形の検出数
（task_core::strategy::contains_secret の正規表現を Rust の source から読む）、run の記録の mutation の有無を <out>/metrics.json に
書く（同じ入力で同じ byte。時刻を書かない）。漏れの規則は子 0 の replay.py・research_case.py と driver の時点を import して使う。
exit: 0 = 漏れが無い、1 = input/ か出力に漏れ（sentinels.json の語・研究ケースの SENTINELS・answer key の印・cutoff 後の event・
禁じた読み）がある、または記録を読めない、2 = 使い方の誤り。漏れがあっても metrics.json は書く。
"""
import argparse
import hashlib
import importlib.util
import json
import pathlib
import re
import sys

FORBIDDEN = ("agent-docs/adr/", "replay/", "parent_checkpoint_", "strategy-v1-6", "execution-plan")
# reviewer の道具が読むことのある system の path（評価の材料ではない）。
SYSTEM = ("/usr/", "/bin/", "/lib/", "/lib64/", "/etc/", "/dev/null", "/proc/self/")


def inside(path, roots):
    p = pathlib.PurePosixPath(path)
    return any(p == pathlib.PurePosixPath(r) or pathlib.PurePosixPath(r) in p.parents for r in roots)


def check_reads(reads, extra=()):
    work, home = reads.get("work_tree"), reads.get("home")
    roots = [r for r in (work, home) if r]
    forbidden, outside = [], []
    for r in reads.get("reads", []):
        text = r.get("path") or r.get("command") or r.get("pattern") or ""
        for name in (*FORBIDDEN, *extra):
            if name in text:
                forbidden.append({"read": text, "name": name})
        if "path" in r:
            path = r["path"] if r["path"].startswith("/") else f"{work}/{r['path']}"
            if not inside(path, roots) and not path.startswith(SYSTEM):
                outside.append(r["path"])
        else:
            for tok in re.findall(r"(?<![\w.])/[^\s'\"|;&<>()]+", text):
                if not inside(tok, roots) and not tok.startswith(SYSTEM):
                    outside.append(tok)
    return forbidden, sorted(set(outside))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--answer-key", required=True)
    ap.add_argument("--pilot", action="store_true")
    ap.add_argument("--runs", action="append", default=[])
    ap.add_argument("--research-src")
    a = ap.parse_args()
    if a.pilot:
        return pilot_main(a)
    if a.runs or a.research_src:
        ap.error("--runs and --research-src need --pilot")
    out = pathlib.Path(a.out)
    key = json.loads(pathlib.Path(a.answer_key).read_text())["checkpoints"]
    points = sorted(str(p.parent.relative_to(out)) for p in out.glob("*/*/*/run.json")) if out.is_dir() else []
    if not points:
        print(f"parent-eval-blind-compare: no <case>/<point>/<arm>/run.json under {out}", file=sys.stderr)
        return 1
    result, failed = {}, []
    for point in points:
        d = out / point
        row = {}
        try:
            output = json.loads((d / "output.json").read_text())
            reads = json.loads((d / "reads.json").read_text())
            run = json.loads((d / "run.json").read_text())
        except (OSError, ValueError) as e:
            failed.append(f"{point}: {e.__class__.__name__}")
            result[point] = {"error": "output.json・reads.json・run.json を読めない"}
            continue
        record = output.get("record") or {"status": output.get("status"),
                                          "effective": {"verdict": output.get("verdict"), "next": output.get("next")}}
        eff = record.get("effective") or {}
        ideal = key.get(output.get("point")) or {}
        forbidden, outside = check_reads(reads)
        row.update({
            "harness": run.get("harness"), "model": run.get("model"), "usage": run.get("usage"), "usd": run.get("usd"),
            "status": record.get("status"), "violations": record.get("violations"),
            "verdict": eff.get("verdict"), "next": eff.get("next"),
            "expected_verdict": ideal.get("ideal_verdict"), "expected_next": ideal.get("ideal_action"),
            "reads": len(reads.get("reads", [])), "forbidden_reads": forbidden, "reads_outside": outside,
        })
        row["verdict_match"] = row["verdict"] is not None and row["verdict"] == row["expected_verdict"]
        row["next_match"] = row["next"] is not None and row["next"] == row["expected_next"]
        if output.get("arm") != "a" and record.get("status") != "checked":
            failed.append(f"{point}: status {record.get('status')}")
        if forbidden or outside:
            failed.append(f"{point}: forbidden or outside reads")
        result[point] = row
    doc = {"schema": "celeris.parent-eval-blind-compare/1", "points": result, "failed": failed,
           "pass": not failed}
    (out / "compare.json").write_text(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    for point, row in result.items():
        print(f"{point}: status={row.get('status')} verdict={row.get('verdict')} next={row.get('next')} "
              f"match={row.get('verdict_match')}/{row.get('next_match')} forbidden={len(row.get('forbidden_reads') or [])} "
              f"outside={len(row.get('reads_outside') or [])}")
    if failed:
        print("parent-eval-blind-compare: FAIL: " + "; ".join(failed), file=sys.stderr)
        return 1
    return 0


# ---------------------------------------------------------------- 対照 pilot（--pilot。ADR 2026-11-24-strategy-pilot-ablation D4・D6）
REPO = pathlib.Path(__file__).resolve().parents[2]
CHILD0 = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0"
SHADOW = REPO / "crates/task-api/tests/fixtures/parent_checkpoint_shadow"
STRATEGY_RS = REPO / "crates/task-core/src/strategy/mod.rs"
PILOT_SCHEMA = "celeris.pilot-metrics/1"
VERDICTS = ("on_track", "at_risk", "off_track", "unknown")
NEXTS = ("continue", "discriminating_experiment", "pivot", "ask_human")
PIVOTS = ("pivot", "discriminating_experiment")
DETECT_VERDICTS = ("at_risk", "off_track")
# 評価器・版を書く run が読んではいけない名（子 6 の FORBIDDEN に足す。ADR D4.1 の読み）。
PILOT_FORBIDDEN = ("answer_key", "answer-key", "research-case.md", "c-results.md", "pilot-key")
OUTPUT_FILES = ("output.json", "reads.json", "run.json", "strategy-v1.json", "strategy-v1.critic.json")
WRITE_TOOLS = ("Write", "Edit", "MultiEdit", "NotebookEdit")
REVIEW_OUTPUT = "review.json"
PRE_FIX = "（Outcome の記録は無い）"
TS_RE = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z")
REDIRECT_RE = re.compile(r">>?\s*([^\s;&|<>]+)")


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def pilot_modules():
    """子 0 の replay.py と、driver（時点・方式・版の段の名。research.py 経由で research_case.py）を import する（規則を写さない）。"""
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / "parent-eval-blind"))
    import driver  # noqa: E402
    import research  # noqa: E402
    return load_module("replay", CHILD0 / "replay/replay.py"), research, driver


def secret_pattern():
    """task_core::strategy::contains_secret の正規表現（`static SECRET` の literal）を Rust の source から読む（Python に写さない）。"""
    m = re.search(r'static SECRET:.*?Regex::new\(\s*r"([^"]+)"', STRATEGY_RS.read_text(), re.S)
    if not m:
        raise SystemExit(f"parent-eval-blind-compare: no `static SECRET` regex in {STRATEGY_RS}")
    return re.compile(m.group(1))


def reference_cells(path):
    """research-case.md の「時点ごとの採点の参照」の表の本文の cell（8 字以上。見出しの行と区切りの行は除く）。"""
    part = path.read_text().split("## 時点ごとの採点の参照", 1)[1].split("\n## ", 1)[0]
    rows = [line for line in part.splitlines() if line.startswith("|")][2:]
    return [c.strip() for row in rows for c in row.strip().strip("|").split("|")[1:] if len(c.strip()) >= 8]


def sha256_file(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


class Rules:
    """漏れの規則（ADR D4.1）。replay は sentinels.json の語・answer key の印と理由の文・期待の組・cutoff 後の event の時刻と本文
    （子 6 の試験 parent_checkpoint_blind_no_answer_key_or_future と同じ。ただし本文は cutoff 以前の event の本文にも同じ文字列が
    あるものを除く。D4.1 付記 1）、研究ケースは research_case.leaks・後の節の見出し・参照の表の文。"""

    def __init__(self, replay, research, key, research_src):
        self.replay, self.research, self.rc = replay, research, research.rc
        self.key = key
        self.events = replay.load_events()
        self.cutoffs = {c["id"]: replay.norm(c["cutoff"]) for c in replay.checkpoints(self.events)}
        sent = json.loads((CHILD0 / "replay/sentinels.json").read_text())
        self.sentinels = [(s["text"], s["first_seen"]) for s in sent["sentinels"]]
        self.whys = [c["why"] for c in key["checkpoints"].values() if c.get("why")]
        self.marks = ("ideal_verdict", "ideal_action", "answer_key", replay.ANSWER_KEY_MARK)
        self.pairs = [f"{v}・{n}" for v in VERDICTS for n in NEXTS]
        self.cells = reference_cells(CHILD0 / "research/research-case.md")
        self.secs = self.rc.sections(pathlib.Path(research_src).read_text()) if research_src else None

    def later(self, point):
        cut = self.cutoffs[point]
        return [e for e in self.events if self.replay.norm(e["ts"]) > cut]

    def leaks(self, case, point, texts):
        found = []
        if case == "replay":
            cut, later = self.cutoffs[point], self.later(point)
            later_ts = {e["ts"] for e in later}
            prior = [e["outcome"] for e in self.events
                     if isinstance(e.get("outcome"), str) and self.replay.norm(e["ts"]) <= cut]
            # 時点以前の本文にも同じ文字列がある本文（定型文など）は、入力にあっても未来の情報と区別できないので数えない（D4.1 付記 1）。
            bodies = {b for b in (e.get("outcome") for e in later)
                      if isinstance(b, str) and len(b) > 40 and not any(b in p for p in prior)}
            for where, name, text in texts:
                at = f"{where}:{name}"
                found += [f"{at}: answer key の印 {m}" for m in self.marks if m in text]
                found += [f"{at}: answer key の理由の文" for w in self.whys if w in text]
                if where == "input":
                    found += [f"{at}: 期待の組 {pair}" for pair in self.pairs if pair in text]
                found += [f"{at}: 未来にだけ現れる語 {t!r}（first_seen {f}）" for t, f in self.sentinels if f > cut and t in text]
                found += [f"{at}: 時点より後の event の時刻 {t}" for t in sorted(later_ts & set(TS_RE.findall(text)))]
                found += [f"{at}: 時点より後の event の本文" for b in bodies if b in text]
                if name.endswith("events.jsonl"):
                    for line in text.splitlines():
                        try:
                            row = json.loads(line)
                        except ValueError:
                            continue
                        if isinstance(row, dict) and isinstance(row.get("ts"), str) and self.replay.norm(row["ts"]) > cut:
                            found.append(f"{at}: cutoff 後の event {row.get('task')}#{row.get('seq')}")
        else:
            heads = self.research.later_headings(self.secs, point) if self.secs else []
            for where, name, text in texts:
                at = f"{where}:{name}"
                found += [f"{at}: {x}" for x in self.rc.leaks(point, text)]
                found += [f"{at}: 後の節の見出し {h!r}" for h in heads if h in text]
                if where == "input":
                    found += [f"{at}: 参照の表の文" for c in self.cells if c in text]
        return sorted(set(found))

    def worker_runs(self, after, until):
        """replay の史実の worker run（worker_finished）のうち after < ts <= until の数と USD（replay.cost_so_far）。"""
        rows = [e for e in self.events if e["type"] == "worker_finished" and after < self.replay.norm(e["ts"]) <= until]
        return {"worker_runs": len(rows), "usd": self.replay.cost_so_far(rows)}

    def h_done(self):
        return max(self.replay.norm(e["ts"]) for e in self.events if e["task"] == "H" and e["type"] == "integration_merged")


def dir_texts(d):
    """(where, name, text): input/ の全 file と、出力の file。"""
    texts = [("input", p.relative_to(d).as_posix(), p.read_text(errors="replace"))
             for p in sorted((d / "input").rglob("*")) if p.is_file()]
    texts += [("output", n, (d / n).read_text(errors="replace")) for n in OUTPUT_FILES if (d / n).is_file()]
    return texts


def secret_counts(d, pattern):
    """secret の形の数（input/ と、それ以外の run の記録の全 file。daemon.log を含む）。値は書かない。"""
    counts = {"input": 0, "output": 0}
    for p in sorted(d.rglob("*")):
        if p.is_file():
            where = "input" if p.relative_to(d).parts[0] == "input" else "output"
            counts[where] += len(pattern.findall(p.read_text(errors="replace")))
    return counts


def mutation(d, reads):
    """run の記録の mutation: input/files.json の sha256 と input/tree/ の file の食い違い（記録の後に入力が変わった）と、
    run が review.json の外に書いた道具の使用（Write・Edit 等の path と Bash の redirect の先）。"""
    listing = json.loads((d / "input/files.json").read_text())
    tree = d / "input/tree"
    want = {f["path"]: f["sha256"] for f in listing.get("files", [])}
    have = {p.relative_to(tree).as_posix(): sha256_file(p) for p in sorted(tree.rglob("*")) if p.is_file()}
    changed = sorted(k for k in set(want) | set(have) if want.get(k) != have.get(k))
    writes = []
    for r in reads.get("reads", []):
        if r.get("tool") in WRITE_TOOLS and not str(r.get("path", "")).endswith(REVIEW_OUTPUT):
            writes.append(r.get("path"))
        for target in REDIRECT_RE.findall(r.get("command") or ""):
            if not target.endswith(REVIEW_OUTPUT) and target != "/dev/null" and not target.startswith("&"):
                writes.append(target)
    return {"input_changed": changed, "writes": sorted(set(writes)), "mutated": bool(changed or writes)}


def claim_support(claims, run, input_text):
    """根拠の参照が slice の外か無い claim（uncertainty は主張でないので数えない）。slice の中: task・artifact は seed の root と子、
    commit・url・knowledge・chat は値が input/ の本文にあるもの。"""
    ids = {run.get("root_id"), *((run.get("children") or {}).values())} - {None}

    def inside(ref):
        kind = ref.get("kind") if isinstance(ref, dict) else None
        if kind in ("task", "artifact"):
            return ref.get("task_id") in ids
        values = [ref.get(k) for k in ("sha", "url", "path", "message_id")] if kind else []
        return any(isinstance(v, str) and len(v) >= 7 and v in input_text for v in values)

    out = {"claims": 0, "uncertainty": 0, "no_ref": 0, "outside": 0, "unsupported": []}
    for c in claims or []:
        if c.get("kind") == "uncertainty":
            out["uncertainty"] += 1
            continue
        out["claims"] += 1
        refs = [r for r in c.get("refs") or [] if r]
        why = "no_ref" if not refs else ("outside" if not all(inside(r) for r in refs) else None)
        if why:
            out[why] += 1
            out["unsupported"].append({"kind": c.get("kind"), "why": why, "statement": (c.get("statement") or "")[:200]})
    return out


def tasks_at(case, point, research):
    """時点の終端の子の数（replay は fixture の slice.json の children_done、研究ケースは 11.15 節からの節の数）。"""
    if case == "replay":
        return len(json.loads((SHADOW / point / "slice.json").read_text())["children_done"])
    return research.rc.POINTS.index(point) + 1


def pilot_point(d, case, point, arm, rules, key, pattern, research):
    output = json.loads((d / "output.json").read_text())
    reads = json.loads((d / "reads.json").read_text())
    run = json.loads((d / "run.json").read_text())
    texts = dir_texts(d)
    input_text = "\n".join(t for w, _, t in texts if w == "input")
    forbidden, outside = check_reads(reads, PILOT_FORBIDDEN)
    ideal = key["checkpoints"].get(point) if case == "replay" else None
    verdict, nxt = output.get("verdict"), output.get("next")
    row = {
        "verdict": verdict, "next": nxt, "status": output.get("status"),
        "tasks_at_point": tasks_at(case, point, research),
        "claims": claim_support(output.get("claims"), run, input_text),
        "llm_runs": run.get("llm_runs") or 0, "usd": run.get("usd") or 0,
        "reads": len(reads.get("reads", [])), "forbidden_reads": forbidden, "reads_outside": outside,
        "secrets": secret_counts(d, pattern), "mutation": mutation(d, reads),
        "pre_fix_input": PRE_FIX in input_text,
        "leaks": rules.leaks(case, point, texts) + [f"reads: {f['name']} ({f['read']})" for f in forbidden],
    }
    if ideal:
        row.update({"expected_verdict": ideal["ideal_verdict"], "expected_next": ideal["ideal_action"],
                    "verdict_match": verdict == ideal["ideal_verdict"], "next_match": nxt == ideal["ideal_action"],
                    "wrong_pivot": ideal["ideal_verdict"] in ("on_track", "unknown") and nxt in PIVOTS,
                    "miss": ideal["ideal_action"] != "continue" and nxt == "continue"})
    return row


def pilot_stage(d, case, point, rules, pattern):
    run = json.loads((d / "run.json").read_text())
    reads = json.loads((d / "reads.json").read_text())
    forbidden, outside = check_reads(reads, PILOT_FORBIDDEN)
    return {"slice_point": point, "task_status": run.get("task_status"), "llm_runs": run.get("llm_runs") or 0,
            "usd": run.get("usd") or 0, "reads": len(reads.get("reads", [])), "reads_outside": outside,
            "secrets": secret_counts(d, pattern), "mutation": mutation(d, reads),
            "leaks": rules.leaks(case, point, dir_texts(d)) + [f"reads: {f['name']} ({f['read']})" for f in forbidden]}


def arm_summary(case, arm, rows, stage, rules, key, order):
    """方式の事例ごとの指標: 未達発見までの task 数、検知の後と、検知できた時点から検知までの史実の worker run・USD（replay）、
    誤った pivot・見逃し・一致の数、usage。"""
    pts = [(p, rows[p]) for p in order if p in rows]
    det = next((p for p, r in pts if r["verdict"] in DETECT_VERDICTS and r["next"] != "continue"), None)
    out = {"points": [p for p, _ in pts], "first_detection": det,
           "tasks_to_detection": rows[det]["tasks_at_point"] if det else (8 if case == "replay" else None),
           "llm_runs": sum(r["llm_runs"] for _, r in pts) + (stage or {}).get("llm_runs", 0),
           "usd": round(sum(r["usd"] for _, r in pts) + (stage or {}).get("usd", 0), 6),
           "unsupported_claims": sum(r["claims"]["no_ref"] + r["claims"]["outside"] for _, r in pts),
           "secrets": sum(r["secrets"]["input"] + r["secrets"]["output"] for _, r in pts),
           "mutated": any(r["mutation"]["mutated"] for _, r in pts)}
    if case == "replay":
        h_done = rules.h_done()
        first = rules.cutoffs[key["first_detectable"]["checkpoint"]]
        cut = rules.cutoffs[det] if det else None
        out["after_detection"] = rules.worker_runs(cut, h_done) if cut else None
        out["unneeded_after_first_detectable"] = rules.worker_runs(first, min(cut, h_done) if cut else h_done)
        out["wrong_pivots"] = sum(1 for _, r in pts if r.get("wrong_pivot"))
        out["misses"] = sum(1 for _, r in pts if r.get("miss"))
        out["verdict_match"] = sum(1 for _, r in pts if r.get("verdict_match"))
        out["next_match"] = sum(1 for _, r in pts if r.get("next_match"))
    return out


DEFINITIONS = {
    "first_detection": "verdict が at_risk・off_track で next が continue 以外の最初の時点（方式 a は verdict が無いので無し）",
    "tasks_to_detection": "first_detection の時点の終端の子の数。replay で検知が無ければ史実の 8（親の closeout）、研究ケースは null",
    "after_detection": "replay の史実で first_detection の cutoff より後、H の統合までに終わった worker run と USD（list price）",
    "unneeded_after_first_detectable": "answer key の first_detectable の cutoff より後、first_detection（無ければ H の統合）までの史実の worker run と USD。rubric のコストの史実は 137 run・264.25 USD",
    "wrong_pivot": "answer key で on_track・unknown の時点に pivot・discriminating_experiment",
    "miss": "answer key の ideal_action が continue 以外の時点に continue",
    "unsupported": "uncertainty を除く claim のうち、参照が無い（no_ref）か slice の外を指す（outside）もの",
    "secrets": "task_core::strategy::contains_secret の正規表現の一致の数（値は書かない）",
    "mutation": "input/files.json と input/tree/ の sha256 の食い違い、run が review.json の外に書いた道具の使用",
    "pre_fix_input": "input/ に「（Outcome の記録は無い）」がある（評価器の入力の修正の前。採点から外す。ADR D8）",
}


def pilot_main(a):
    if not a.runs:
        print("parent-eval-blind-compare: --pilot needs --runs <dir>", file=sys.stderr)
        return 2
    replay, research, driver = pilot_modules()
    from pilot_runs import iteration_roots
    try:
        roots = iteration_roots(a.runs)
    except ValueError as e:
        print(f"parent-eval-blind-compare: {e}", file=sys.stderr)
        return 2
    names = [root.resolve().name for root in roots]
    key = json.loads(pathlib.Path(a.answer_key).read_text())
    rules = Rules(replay, research, key, a.research_src)
    pattern = secret_pattern()
    doc = {"schema": PILOT_SCHEMA, "answer_key_sha256": sha256_file(a.answer_key), "definitions": DEFINITIONS,
           "history": {"first_detectable": key["first_detectable"]["checkpoint"],
                       **rules.worker_runs(rules.cutoffs[key["first_detectable"]["checkpoint"]], rules.h_done())},
           "iterations": {}, "agreement": {}, "leaks": [], "excluded": [], "failed": []}
    for name, root in zip(names, roots):
        it = {"points": {}, "strategy": {}, "arms": {}}
        found = sorted(p.parent.relative_to(root) for p in root.glob("*/*/*/run.json")) if root.is_dir() else []
        found += sorted(p.parent.relative_to(root) for p in root.glob(f"*/{driver.STRATEGY_STAGE}/run.json"))
        if not found:
            doc["failed"].append(f"{name}: no <case>/<point>/<arm>/run.json")
        for rel in found:
            parts, label = rel.parts, f"{name}/{rel.as_posix()}"
            case = parts[0]
            try:
                if parts[1] == driver.STRATEGY_STAGE:
                    if case not in driver.STRATEGY_POINT:
                        raise ValueError(f"unknown case {case}")
                    row = pilot_stage(root / rel, case, driver.STRATEGY_POINT[case], rules, pattern)
                    it["strategy"][case] = row
                else:
                    point, arm = parts[1], parts[2]
                    if point not in driver.CASES.get(case, ()) or arm not in driver.ARMS:
                        raise ValueError(f"not a pilot point or arm: {rel}")
                    row = pilot_point(root / rel, case, point, arm, rules, key, pattern, research)
                    it["points"][rel.as_posix()] = row
                    if row["pre_fix_input"]:
                        doc["excluded"].append(f"{label}: 評価器の入力の修正の前（{PRE_FIX}）")
            except (OSError, ValueError, KeyError) as e:
                doc["failed"].append(f"{label}: {e.__class__.__name__}: {e}")
                continue
            doc["leaks"] += [f"{label}: {x}" for x in row["leaks"]]
            if row["leaks"]:
                doc["excluded"].append(f"{label}: 漏れ")
        for case, order in driver.CASES.items():
            for arm in driver.ARMS:
                rows = {k.split("/")[1]: v for k, v in it["points"].items() if k.startswith(f"{case}/") and k.endswith(f"/{arm}")}
                if rows:
                    stage = it["strategy"].get(case) if arm == "c" else None
                    it["arms"].setdefault(arm, {})[case] = arm_summary(case, arm, rows, stage, rules, key, order)
        it["usage"] = {"llm_runs": sum(r["llm_runs"] for r in [*it["points"].values(), *it["strategy"].values()]),
                       "usd": round(sum(r["usd"] for r in [*it["points"].values(), *it["strategy"].values()]), 6)}
        doc["iterations"][name] = it
    keys = sorted({k for it in doc["iterations"].values() for k in it["points"]})
    for k in keys:
        seen = [(it["points"][k]["verdict"], it["points"][k]["next"]) for it in doc["iterations"].values() if k in it["points"]]
        if len(seen) > 1:
            doc["agreement"][k] = {"iterations": len(seen), "same": len(set(seen)) == 1}
    doc["usage"] = {"llm_runs": sum(it["usage"]["llm_runs"] for it in doc["iterations"].values()),
                    "usd": round(sum(it["usage"]["usd"] for it in doc["iterations"].values()), 6)}
    doc["pass"] = not doc["leaks"] and not doc["failed"]
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "metrics.json").write_text(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    for name, it in doc["iterations"].items():
        for arm, cases in sorted(it["arms"].items()):
            for case, s in sorted(cases.items()):
                print(f"{name} {case} arm {arm}: detection={s['first_detection']} tasks={s['tasks_to_detection']} "
                      f"runs={s['llm_runs']} usd={s['usd']} unsupported={s['unsupported_claims']} secrets={s['secrets']}")
    if doc["failed"] or doc["leaks"]:
        print("parent-eval-blind-compare: FAIL: " + "; ".join(doc["failed"] + doc["leaks"])[:4000], file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
