#!/usr/bin/env python3
"""CoS chat live bench の driver（scripts/dev/cos-chat-bench.sh から呼ばれる。ADR 2026-10-08-cos-chat-prompt-cache D3.4-2）。

台本 scripts/dev/cos-chat-bench-script.json を隔離 daemon の API で流し、chat run ごとの telemetry
（GET /chat/threads/{t}/runs/{r}）と node_sessions.approx_tokens・prompt.txt の bytes を集める。
出力: <evidence>/runs.json・runs.csv・summary.json・tables.md。測れない値は null（表では「不明」）。
"""
import csv, glob, json, os, sqlite3, statistics, subprocess, sys, time, urllib.request

api_base, token, out, repo, mode, reduced, claude_version = sys.argv[1:8]
reduced = reduced == "1"
EV = os.path.join(out, "evidence")
SCRIPT = json.load(open(os.path.join(repo, "scripts/dev/cos-chat-bench-script.json")))
DB = os.path.join(out, "celeris.sqlite3")


def call(method, path, body=None):
    req = urllib.request.Request(api_base + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Authorization": "Bearer " + token, "content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or b"null")


def log(msg):
    line = time.strftime("%H:%M:%S", time.gmtime()) + " " + msg
    print(line, flush=True)
    open(os.path.join(EV, "steps.log"), "a").write(line + "\n")


TERMINAL = {"completed", "failed", "cancelled", "stopped", "interrupted"}
runs = []
counter = [0]


def new_thread(tag):
    counter[0] += 1
    return call("POST", "/chat/threads", {"title": "bench " + tag, "project_id": None,
                                          "client_thread_id": f"bench-{os.getpid()}-{counter[0]}"})["thread"]["id"]


def run_dir(run_id):
    for p in glob.glob(os.path.join(out, "**", "prompt.txt"), recursive=True):
        try:
            with open(p, "rb") as f:
                head = f.read(600)
        except OSError:
            continue
        if f"chat run {run_id}".encode() in head:
            return os.path.dirname(p)
    return None


def enrich(rec):
    """prompt.txt の bytes と stdout.jsonl（stream-json）の model・5m/1h 内訳・名目 cost・init の道具/skill 数。"""
    d = run_dir(rec["run_id"]) if rec.get("run_id") else None
    if not d:
        return
    rec["prompt_bytes"] = os.path.getsize(os.path.join(d, "prompt.txt"))
    try:
        for line in open(os.path.join(d, "stdout.jsonl")):
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if e.get("type") == "system" and e.get("subtype") == "init":
                rec["model"] = rec.get("model") or e.get("model")
                rec["init_tools"] = len(e.get("tools") or [])
                rec["init_skills"] = len(e.get("skills") or [])
                rec["init_mcp_servers"] = len(e.get("mcp_servers") or [])
            if e.get("type") == "result":
                cc = (e.get("usage") or {}).get("cache_creation") or {}
                rec["cache_creation_5m"] = cc.get("ephemeral_5m_input_tokens")
                rec["cache_creation_1h"] = cc.get("ephemeral_1h_input_tokens")
                rec["cli_total_cost_usd"] = e.get("total_cost_usd")
                rec["api_iterations"] = len((e.get("usage") or {}).get("iterations") or []) or None
    except OSError:
        pass


def approx_tokens(thread):
    try:
        c = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
        row = c.execute("select approx_tokens from node_sessions where thread_id=? order by rowid desc limit 1", (thread,)).fetchone()
        return row[0] if row else None
    except sqlite3.Error:
        return None


def turn(script, group, thread, idx, kind, text, gap_label=None):
    key = f"{group}-{counter[0]}-{idx}"
    t0 = time.time()
    call("POST", f"/chat/threads/{thread}/messages", {"client_message_id": key, "text": text, "attachment_ids": [],
                                                      "reply_to_id": None, "mode": "queue", "resume_queue": False})
    run_id = None
    while time.time() - t0 < 600 and not run_id:
        items = call("GET", f"/chat/threads/{thread}/messages?limit=200")["items"]
        run_id = next((m.get("run_id") for m in items if m.get("client_message_id") == key and m.get("run_id")), None)
        if not run_id:
            time.sleep(2)
    rec = {"group": group, "thread": thread, "turn": idx, "kind": kind, "gap_secs": gap_label, "run_id": run_id,
           "sent_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t0))}
    if not run_id:
        rec.update(state="no_run")
        runs.append(rec); log(f"{key}: no run id"); return
    run = {}
    while time.time() - t0 < 900:
        run = call("GET", f"/chat/threads/{thread}/runs/{run_id}")["run"]
        if run["state"] in TERMINAL:
            break
        time.sleep(3)
    wall = time.time() - t0
    u = run.get("usage") or {}
    cfg = {}
    rec.update(state=run["state"], reason=run.get("reason"), harness=run.get("harness"), llm_source=run.get("llm_source"),
               account_id=run.get("account_id"), model=run.get("model"), session_mode=run.get("session_mode"),
               input_tokens=u.get("input_tokens"), cache_read_tokens=u.get("cache_read_tokens"),
               cache_creation_tokens=u.get("cache_creation_tokens"), output_tokens=u.get("output_tokens"),
               cost_usd=u.get("cost_usd"), session_resumed=u.get("session_resumed"), duplicate_reads=u.get("duplicate_reads"),
               skill_reads=run.get("skill_reads"), latency_ms=run.get("latency_ms"),
               time_to_first_output_ms=run.get("time_to_first_output_ms"), wall_ms_observed=int(wall * 1000),
               approx_tokens_after=approx_tokens(thread))
    if all(rec.get(k) is not None for k in ("input_tokens", "cache_read_tokens", "cache_creation_tokens")):
        rec["context_tokens"] = rec["input_tokens"] + rec["cache_read_tokens"] + rec["cache_creation_tokens"]
    enrich(rec)
    runs.append(rec)
    json.dump(runs, open(os.path.join(EV, "runs.json"), "w"), ensure_ascii=False, indent=1)
    log(f"{group} t{idx} {run['state']} in={rec['input_tokens']} cr={rec['cache_read_tokens']} cw={rec['cache_creation_tokens']} out={rec['output_tokens']} lat={rec['latency_ms']}")


def med(vals):
    v = [x for x in vals if x is not None]
    return statistics.median(v) if v else None


def summarize():
    groups = {}
    for r in runs:
        groups.setdefault(r["group"], []).append(r)
    metrics = ["input_tokens", "cache_read_tokens", "cache_creation_tokens", "output_tokens", "latency_ms",
               "time_to_first_output_ms", "skill_reads", "prompt_bytes", "context_tokens", "cost_usd", "cli_total_cost_usd", "cache_creation_5m", "cache_creation_1h", "init_tools", "init_skills", "api_iterations", "approx_tokens_after"]
    summary = {}
    for g, rs in groups.items():
        ok = sum(1 for r in rs if r.get("state") == "completed")
        summary[g] = {"runs": len(rs), "completed": ok, "success_rate": ok / len(rs),
                      **{"median_" + m: med([r.get(m) for r in rs]) for m in metrics},
                      "unknown_counts": {m: sum(1 for r in rs if r.get(m) is None) for m in metrics}}
    for kind in ("consult", "ops"):
        rs = [r for r in groups.get("s2", []) if r["kind"] == kind]
        if rs:
            summary["s2_" + kind] = {"runs": len(rs), **{"median_" + m: med([r.get(m) for r in rs]) for m in metrics}}
    return summary


def finish():
    summary = summarize()
    meta = {"commit": subprocess.run(["git", "-C", repo, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip(),
            "claude_version": claude_version, "finished_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "mode": mode, "reduced": reduced, "script": "scripts/dev/cos-chat-bench-script.json",
            "harness": sorted({r.get("harness") for r in runs if r.get("harness")}),
            "model": sorted({r.get("model") for r in runs if r.get("model")}),
            "llm_source": sorted({r.get("llm_source") for r in runs if r.get("llm_source")}),
            "account_id": sorted({str(r.get("account_id")) for r in runs}),
            "llm_runs": len(runs), "model_note": "run.model is null when the CLI default is used; model comes from the stream-json init event"}
    json.dump({"meta": meta, "summary": summary, "runs": runs}, open(os.path.join(EV, "summary.json"), "w"), ensure_ascii=False, indent=1)
    cols = list(runs[0].keys()) if runs else []
    for r in runs:
        for k in r:
            if k not in cols:
                cols.append(k)
    with open(os.path.join(EV, "runs.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        w.writeheader(); w.writerows(runs)
    show = ["input_tokens", "cache_read_tokens", "cache_creation_tokens", "output_tokens", "time_to_first_output_ms", "latency_ms", "skill_reads", "prompt_bytes"]
    lines = ["| group | runs | 成功率 | " + " | ".join("median " + m for m in show) + " |", "|" + "---|" * (3 + len(show))]
    for g, s in summary.items():
        cells = [str(s.get("median_" + m)) if s.get("median_" + m) is not None else "不明" for m in show]
        lines.append(f"| {g} | {s['runs']} | {('%.2f' % s['success_rate']) if 'success_rate' in s else '-'} | " + " | ".join(cells) + " |")
    open(os.path.join(EV, "tables.md"), "w").write("\n".join(lines) + "\n")
    log("summary written")


def main():
    n_s1 = SCRIPT["reduced"]["s1_new_threads"] if reduced else SCRIPT["s1_new_threads"]["count"]
    n_s2 = SCRIPT["reduced"]["s2_turns"] if reduced else len(SCRIPT["s2_same_thread"]["turns"])
    if mode == "enrich":  # 既存の runs.json に enrich をかけ直して summary を作り直す（LLM は呼ばない）
        runs.extend(json.load(open(os.path.join(EV, "runs.json"))))
        for r in runs:
            enrich(r)
        finish(); return
    if mode == "dry":
        log("dry: thread " + new_thread("dry")); return
    if mode == "cold-ttl":  # 1h TTL を超えた cold（台本 s6_cold_ttl、LLM run 4 回）
        c = SCRIPT["s6_cold_ttl"]
        try:
            th = new_thread("s6")
            turn(SCRIPT, "s6_seed", th, 1, "consult", c["text"])
            log(f"s6: waiting {c['gap_secs']} s for the cache TTL to pass")
            time.sleep(c["gap_secs"])
            turn(SCRIPT, "s6_cold_resumed", th, 2, "consult", c["text"], c["gap_secs"])
            turn(SCRIPT, "s6_cold_new_thread", new_thread("s6-new"), 1, "consult", SCRIPT["consult"], c["gap_secs"])
            time.sleep(c["warm_gap_secs"])
            turn(SCRIPT, "s6_warm_resumed", th, 3, "consult", c["text"], c["warm_gap_secs"])
        finally:
            if runs:
                finish()
        return
    try:
        for i in range(1, n_s1 + 1):
            turn(SCRIPT, "s1", new_thread(f"s1-{i}"), 1, "consult", SCRIPT["consult"])
        th = new_thread("s2")
        for i, t in enumerate(SCRIPT["s2_same_thread"]["turns"][:n_s2], 1):
            turn(SCRIPT, "s2", th, i, t["kind"], t["text"])
        if not reduced:
            c = SCRIPT["s5_cold_warm"]
            idx = n_s2
            for _ in range(c["warm_turns"]):
                time.sleep(c["warm_gap_secs"]); idx += 1
                turn(SCRIPT, "s5_warm", th, idx, "consult", c["text"], c["warm_gap_secs"])
            for _ in range(c["cold_turns"]):
                time.sleep(c["cold_gap_secs"]); idx += 1
                turn(SCRIPT, "s5_cold", th, idx, "consult", c["text"], c["cold_gap_secs"])
    finally:
        if runs:
            finish()


main()
