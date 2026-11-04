#!/usr/bin/env python3
"""P2-G の課題の入力（期待差分 fixture の元）を旧挙動の fixture から作る（ADR 2026-11-03-llm-self-improve-trial）。
  python3 -I scripts/dev/llm-improve/make-inputs.py <課題 dir>
課題 dir の名前（T1-cap-background・T2-cap-item-title）で入力の作り方を選び、<課題 dir>/inputs.jsonl を書く。
旧 fixture の文字列は最長 11 文字なので、下の長さ（80 文字以上）の入力は旧 fixture に当たらない。
"""
import copy, json, pathlib, sys

root = pathlib.Path(__file__).resolve().parents[3]
rows = [json.loads(l) for l in open(root / "control-ts/fixtures/cos_digest/cases.jsonl", encoding="utf-8")]
task = pathlib.Path(sys.argv[1]).resolve()
out = []

def pick(pred, n):
    got = [copy.deepcopy(r["input"]) for r in rows if pred(r["input"])][:n]
    assert len(got) == n, "fixture に足りる入力が無い"
    return got

def escalated_with_background(i, idx=0, n_items=1):
    if len(i["items"]) != n_items:
        return False
    it = i["items"][idx]
    d = it["decision"]
    return it["state"] in ("escalated", "fallback") and d is not None and d["background"] not in (None, "")

if task.name.startswith("T1-"):
    base = pick(lambda i: escalated_with_background(i), 3)
    for k, b in enumerate(base):
        for n in (150, 300, 301, 1000):
            x = copy.deepcopy(b); x["items"][0]["decision"]["background"] = "B" * n
            out.append((f"t1/single-{k}/background-{n}", x))
    x = copy.deepcopy(base[0]); x["items"][0]["decision"]["background"] = "あ" * 301
    out.append(("t1/kana-301", x))
    x = copy.deepcopy(base[1]); x["items"][0]["decision"]["background"] = "😀" * 400
    out.append(("t1/emoji-400", x))
    two = pick(lambda i: escalated_with_background(i, 0, 2), 1)[0]
    two["items"][0]["decision"]["background"] = "B" * 500
    out.append(("t1/two-items-first-long", two))
    ans = pick(lambda i: len(i["items"]) == 1 and i["items"][0]["state"] == "answered"
               and i["items"][0]["decision"] is not None, 1)[0]
    ans["items"][0]["decision"]["background"] = "B" * 500
    out.append(("t1/answered-long-background-not-shown", ans))
elif task.name.startswith("T2-"):
    base = pick(lambda i: len(i["items"]) == 1 and i["items"][0]["summary"].strip() != "", 3)
    for k, b in enumerate(base):
        for n in (80, 81, 300):
            x = copy.deepcopy(b); x["items"][0]["summary"] = "S" * n
            out.append((f"t2/single-{k}/summary-{n}", x))
    x = copy.deepcopy(base[0]); x["items"][0]["summary"] = "あ" * 120
    out.append(("t2/kana-120", x))
    x = copy.deepcopy(base[1]); x["items"][0]["summary"] = "😀" * 81
    out.append(("t2/emoji-81", x))
    x = copy.deepcopy(base[2]); x["items"][0]["summary"] = " " * 100
    out.append(("t2/blank-summary-uses-kind-key", x))
else:
    sys.exit(f"unknown task dir: {task.name}")

with open(task / "inputs.jsonl", "w", encoding="utf-8") as f:
    for name, x in out:
        f.write(json.dumps({"name": name, "input": x}, ensure_ascii=False) + "\n")
print(f"{task.name}: {len(out)} inputs")
