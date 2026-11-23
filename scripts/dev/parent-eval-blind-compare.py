#!/usr/bin/env python3
"""盲検の評価の出力を比べる（scripts/dev/parent-eval-blind.sh の後に人が走らせる）。

  python3 -I scripts/dev/parent-eval-blind-compare.py --out <dir> --answer-key <answer key の json>

<dir>/<point>/evaluation.json（最終の記録）・reads.json・run.json を読み、時点ごとに記録の status・verdict・next と、answer key
の期待との一致を出す。reads に作業 tree・一時 home の外の path、または禁じた名前（評価器に見せない文書の印）があれば
不合格。結果を <dir>/compare.json に書く（同じ入力で同じ byte。時刻を書かない）。
exit: 0 = 全時点の評価が checked で禁じた読みが無い（不一致でも 0）、1 = 評価が無い・checked でない・禁じた読み、2 = 使い方の誤り。
"""
import argparse
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


def check_reads(reads):
    work, home = reads.get("work_tree"), reads.get("home")
    roots = [r for r in (work, home) if r]
    forbidden, outside = [], []
    for r in reads.get("reads", []):
        text = r.get("path") or r.get("command") or r.get("pattern") or ""
        for name in FORBIDDEN:
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
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    key = json.loads(pathlib.Path(a.answer_key).read_text())["checkpoints"]
    points = sorted(p.name for p in out.iterdir() if p.is_dir() and (p / "run.json").exists()) if out.is_dir() else []
    if not points:
        print(f"parent-eval-blind-compare: no <point>/run.json under {out}", file=sys.stderr)
        return 1
    result, failed = {}, []
    for point in points:
        d = out / point
        row = {}
        try:
            record = json.loads((d / "evaluation.json").read_text())
            reads = json.loads((d / "reads.json").read_text())
            run = json.loads((d / "run.json").read_text())
        except (OSError, ValueError) as e:
            failed.append(f"{point}: {e.__class__.__name__}")
            result[point] = {"error": "evaluation.json・reads.json・run.json を読めない"}
            continue
        eff = record.get("effective") or {}
        ideal = key.get(point) or {}
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
        if record.get("status") != "checked":
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


if __name__ == "__main__":
    sys.exit(main())
