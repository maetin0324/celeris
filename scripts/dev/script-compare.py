#!/usr/bin/env python3
"""TS 制御層の本番 compare の preflight・集計・標本取り（読み取り専用。境界 ADR 付記 P1-E）。

使い方（path に既定値は無い。本番を指すのは人が渡す引数だけ）:
  python3 -I scripts/dev/script-compare.py preflight --config <toml> --state-dir <dir> --measure-root <dir>...
      [--measure-bin <dir>]... [--proc-root <dir>] [--repo <git の木>] [--estimate-since <rfc3339>]
  python3 -I scripts/dev/script-compare.py report --db <sqlite> [--samples <jsonl>]... [--tick-log <file>]... [--upto-event-id N] [--upto-log-id N]
      [--require-ready <domain,...>] [--format json|table]
  python3 -I scripts/dev/script-compare.py sample --pid <pid> (--daemon-json <file> | --api <origin> --token-file <file>)
      [--count N] [--interval-secs S] [--proc-root <dir>]

何も書かない:
- DB は sqlite3 CLI を `-readonly` と `PRAGMA query_only=ON` で開き、SELECT と読み取りの PRAGMA だけを流す。
- 測定環境の DB は開かない（config の file を読み、file の同一性を stat で比べるだけ）。
- 件数を足す経路は無い。集計は `events` の `script_compare_recorded` と `script_compare_log` の行だけから数える。
  `script_guard_state` の計数は生の記録と照合するためだけに読み、合わなければ problem にして ready にしない。

report の exit: 0 = 読めて矛盾なし（--require-ready があれば全部 ready）。1 = problem あり（未知の outcome・
正規化の記録・計数の不一致など）。2 = 引数・読み取りの誤り。3 = 表が無い（migration の前）。4 = 矛盾は無いが
--require-ready の領域が ready でない。
preflight の exit: 0 = 全部 pass。1 = fail あり（分離の破れなど。人の手順に進まない）。2 = 引数・読み取りの誤り。
3 = fail は無いが human の項目（release・config など人の手順）が残っている。

report は 1 つの read transaction で読むので、動いている DB でも記録と計数が食い違わない。同じ DB・同じ引数の report は
同じ byte 列を出す（時計を読まない）。`bounds` の id を --upto-* に渡すと動いている DB でも記録の集計は再現する。その場合は
mode・guard の計数・受信箱が今の値で記録と時点が違うので、guard との照合をせず ready にしない。
"""
import argparse
import calendar
import json
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request

# (f) の閾値。正本は crates/task-core/src/script_guard.rs の SCRIPT_GUARD_*。
REQUIRED = {"lane_rules": 1000, "route_priority": 1000, "cos_digest": 200}
WINDOW = 1000
OUTCOMES = ("match", "mismatch", "ts_error", "invalid_output", "unavailable")
# compare に要る表（0103〜0107）。
TABLES = (
    "script_generations",
    "script_domain_modes",
    "script_domain_events",
    "script_compare_log",
    "script_generation_events",
    "script_guard_state",
    "script_inbox_items",
    "cos_digest_shadow_outbox",
)
MIN_SCHEMA = 107  # 0107_cos_digest_shadow_outbox（P0-B）。これより前の release では compare に上げない。
GATE_STEPS = ("control-ts-install", "control-ts-bundle", "script-bundle-verify", "script-bundle-tests")


class ReadError(Exception):
    pass


def query(db, sql):
    """SELECT を読み取り専用で流し、行（1 列目の文字列）を返す。"""
    if shutil.which("sqlite3") is None:
        raise ReadError("sqlite3 CLI not found")
    if not os.path.exists(db):
        raise ReadError(f"no such db: {db}")
    cmd = ["sqlite3", "-readonly", "-batch", "-bail", "-cmd", ".timeout 15000", "-cmd", "PRAGMA query_only=ON", db, sql]
    p = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    if p.returncode != 0:
        raise ReadError(f"sqlite3 failed on {db}: {p.stderr.strip()}")
    return p.stdout.splitlines()


def query_json(db, sql):
    return [json.loads(line) for line in query(db, sql) if line.strip()]


def nearest_rank(ordered, p):
    return ordered[-(-p * len(ordered) // 100) - 1]


def latency(values):
    if not values:
        return None
    s = sorted(values)
    return {"n": len(s), "p50_us": nearest_rank(s, 50), "p95_us": nearest_rank(s, 95), "max_us": s[-1]}


def missing_tables(db):
    names = set(query(db, "SELECT name FROM sqlite_master WHERE type='table'"))
    return [t for t in TABLES if t not in names]


# --- report ---------------------------------------------------------------------------------------------------------


def classify(rec):
    """S2-8 の分類（ScriptGuardCall::classify と同じ）。unavailable は None。"""
    o = rec["outcome"]
    if o == "match":
        return "ok"
    if o == "mismatch":
        return "mismatch"
    if o == "ts_error":
        return "timeout" if rec["error_kind"] in ("timeout", "terminated") else "failure"
    if o == "invalid_output":
        return "failure"
    return None


def rate_exceeded(count, length):
    return count * 1000 > length


RECORD_FIELDS = """'id',id,'ts',ts,'domain',json_extract(json,'$.domain'),'generation',json_extract(json,'$.generation'),
    'bundle_sha256',json_extract(json,'$.bundle_sha256'),'outcome',json_extract(json,'$.outcome'),
    'error_kind',json_extract(json,'$.error.kind'),'elapsed_us',json_extract(json,'$.elapsed_us'),
    'normalized',json_array_length(json,'$.normalized'),'safety',json_array_length(json,'$.safety_violations'),
    'canary_selected',json_extract(json,'$.canary_selected'),'applied_by',json_extract(json,'$.applied_by'),
    'fallback',json_extract(json,'$.fallback.code'),'rust',json_extract(json,'$.rust'),'ts_out',json_extract(json,'$.ts')"""


def read_snapshot(db, upto_event, upto_log):
    """全部を 1 つの read transaction で読み、表ごとの行を返す。"""
    event_bound = "(SELECT coalesce(max(id),0) FROM events)" if upto_event is None else str(int(upto_event))
    log_bound = "(SELECT coalesce(max(id),0) FROM script_compare_log)" if upto_log is None else str(int(upto_log))
    selects = [
        f"SELECT json_object('t','bounds','upto_event_id',{event_bound},'upto_log_id',{log_bound})",
        """SELECT json_object('t','mode','domain',domain,'mode',mode,'generation',generation_id,'version',version,
        'updated_at',updated_at,'updated_by',updated_by,'canary_bps',canary_bps) FROM script_domain_modes ORDER BY domain""",
        """SELECT json_object('t','guard','domain',domain,'generation',generation_id,'mode',mode,'calls',calls,
        'clean_streak',clean_streak,'ts_side_clean_streak',ts_side_clean_streak,
        'consecutive_failures',consecutive_failures,'safety_violations',safety_violations,
        'halted',halted,'halt_reason',halt_reason,'updated_at',updated_at) FROM script_guard_state
        ORDER BY domain,generation_id""",
        f"""SELECT json_object('t','record',{RECORD_FIELDS}) FROM events WHERE id<={event_bound}
        AND json_extract(json,'$.type')='script_compare_recorded' ORDER BY id""",
        f"SELECT json_object('t','record',{RECORD_FIELDS}) FROM script_compare_log WHERE id<={log_bound} ORDER BY id",
        """SELECT json_object('t','change','id',id,'ts',ts,'domain',domain,'from_mode',json_extract(json,'$.from_mode'),
        'to_mode',json_extract(json,'$.to_mode'),'from_generation',json_extract(json,'$.from_generation'),
        'to_generation',json_extract(json,'$.to_generation'),'actor',json_extract(json,'$.actor'),
        'reason',json_extract(json,'$.reason')) FROM script_domain_events ORDER BY id""",
        """SELECT json_object('t','generation_event','id',id,'ts',ts,'generation',generation_id,
        'type',json_extract(json,'$.type'),'actor',json_extract(json,'$.actor'),'reason',json_extract(json,'$.reason'))
        FROM script_generation_events WHERE json_extract(json,'$.type') IN
        ('script_generation_invalidated','script_guard_halted','script_guard_cleared') ORDER BY id""",
        """SELECT json_object('t','inbox','items',count(*),'open',coalesce(sum(resolved_at IS NULL),0))
        FROM script_inbox_items""",
        "SELECT json_object('t','outbox','pending',count(*)) FROM cos_digest_shadow_outbox",
    ]
    rows = {}
    for row in query_json(db, "BEGIN; " + "; ".join(selects) + "; COMMIT;"):
        rows.setdefault(row.pop("t"), []).append(row)
    return rows


def aggregate(records, problems):
    groups = {}
    for r in records:
        key = (r["domain"] or "?", r["generation"] or "?")
        g = groups.setdefault(
            key,
            {
                "domain": key[0],
                "generation": key[1],
                "bundle_sha256": r["bundle_sha256"],
                "outcomes": {o: 0 for o in OUTCOMES},
                "other_outcomes": {},
                "timeouts": 0,
                "failures": 0,
                "safety_violations": 0,
                "normalized": 0,
                "canary_selected": 0,
                "applied_by_ts": 0,
                "fallbacks": {},
                "hidden_differences": 0,
                "first_ts": r["ts"],
                "last_ts": r["ts"],
                "_lat": [],
                "_window": [],
                "clean_streak": 0,
            },
        )
        g["last_ts"] = r["ts"]  # 表ごとに id 順に読む。
        where = f"{key[0]} id {r['id']}"
        o = r["outcome"]
        if o not in OUTCOMES:
            g["other_outcomes"][str(o)] = g["other_outcomes"].get(str(o), 0) + 1
            problems.append(f"{where}: unknown outcome {o!r}")
            continue
        g["outcomes"][o] += 1
        if r["normalized"] != 0:
            g["normalized"] += 1
            problems.append(f"{where}: normalized is not empty (S2-2 has no normalization rule)")
        if r["canary_selected"]:
            g["canary_selected"] += 1
        if r["applied_by"] == "ts":
            g["applied_by_ts"] += 1
        if r["fallback"]:
            g["fallbacks"][r["fallback"]] = g["fallbacks"].get(r["fallback"], 0) + 1
        # match なのに rust と ts が違う記録は、差分を隠す記録として止める（どの領域も比べた欄の全体を持つ）。
        if o == "match" and r["rust"] != r["ts_out"]:
            g["hidden_differences"] += 1
            problems.append(f"{where}: outcome match but rust != ts")
        call = classify(r)
        if call is None:
            continue
        if not isinstance(r["elapsed_us"], int) or r["elapsed_us"] < 0:
            problems.append(f"{where}: elapsed_us is not a count")
        else:
            g["_lat"].append(r["elapsed_us"])
        g["clean_streak"] = 0 if call == "mismatch" else g["clean_streak"] + 1
        g["timeouts"] += call == "timeout"
        g["failures"] += call == "failure"
        if r["safety"] != 0:
            g["safety_violations"] += 1
        g["_window"].append(call)
    return groups


def finish_group(g, guard, mode, promoted, bounded, problems):
    required = REQUIRED.get(g["domain"])
    if required is None:
        problems.append(f"unknown domain {g['domain']!r}")
        required = 0
    window = g.pop("_window")[-WINDOW:]
    lat = g.pop("_lat")
    out = g["outcomes"]
    calls = sum(out[o] for o in OUTCOMES if o != "unavailable")
    t_win, f_win = window.count("timeout"), window.count("failure")
    g["calls"] = calls
    g["latency"] = latency(lat)
    denom = out["match"] + out["mismatch"] + out["invalid_output"] + out["ts_error"]
    g["match_rate"] = round(out["match"] / denom, 6) if denom else None
    g["window"] = {"len": len(window), "timeouts": t_win, "failures": f_win}
    g["required"] = required
    current = mode is not None and mode["generation"] == g["generation"]
    g["current_mode"] = mode["mode"] if current else None
    g["guard_state"] = guard
    # 計数と生の記録の照合（計数だけを書いた DB を ready にしない）。canary・ts を通った generation は guard が
    # mode の変わり目で数え直すので、記録より多いときだけ破れとする。
    streak = g["clean_streak"]
    guard_ok = True
    if bounded:
        pass
    elif guard is None:
        guard_ok = calls == 0
        if not guard_ok:
            problems.append(f"{g['domain']} {g['generation']}: no script_guard_state row for {calls} calls")
    else:
        restarted = (g["domain"], g["generation"]) in promoted
        if restarted:
            guard_ok = guard["calls"] <= calls and guard["clean_streak"] <= streak
            streak = guard["clean_streak"]
        else:
            guard_ok = guard["calls"] == calls and guard["clean_streak"] == streak
        if not guard_ok:
            problems.append(
                f"{g['domain']} {g['generation']}: script_guard_state (calls {guard['calls']}, clean_streak "
                f"{guard['clean_streak']}) disagrees with the records (calls {calls}, clean_streak {g['clean_streak']})"
            )
    blockers = []
    if bounded:
        blockers.append("bounded replay: guard state is not from the same moment")
    if not current:
        blockers.append("not the generation of the current mode")
    elif mode["mode"] != "compare":
        # canary→ts の前提は TS 側の単位の計数で、記録からは決められない（ts_side は記録の欄でない）。
        blockers.append(f"mode is {mode['mode']}: this gate is compare→canary (see guard_state.ts_side_clean_streak)")
    if streak < required:
        blockers.append(f"clean_streak {streak} < {required}")
    if rate_exceeded(t_win, len(window)):
        blockers.append(f"timeouts {t_win} in window {len(window)}")
    if rate_exceeded(f_win, len(window)):
        blockers.append(f"failures {f_win} in window {len(window)}")
    if g["safety_violations"]:
        blockers.append(f"safety_violations {g['safety_violations']}")
    if g["normalized"] or g["other_outcomes"] or g["hidden_differences"]:
        blockers.append("unknown outcome, normalized record or match with different outputs")
    if not guard_ok:
        blockers.append("guard state disagrees with the records")
    g["effective_clean_streak"] = streak
    g["ready_blockers"] = blockers
    g["ready"] = not blockers
    return g


def rollback_summary(rows):
    changes = rows.get("change", [])
    automatic, by_actor, promoted = [], {}, set()
    for r in changes:
        if r["to_mode"] in ("canary", "ts"):
            promoted.add((r["domain"], r["to_generation"]))
        if r["to_mode"] == "rust" and r["from_mode"] != "rust":
            by_actor[r["actor"]] = by_actor.get(r["actor"], 0) + 1
            if str(r["actor"]).startswith("system:"):
                automatic.append(r)
    inbox = rows["inbox"][0]
    return (
        {
            "mode_changes": changes,
            "to_rust_by_actor": by_actor,
            "automatic_rollbacks": automatic,
            "generation_events": rows.get("generation_event", []),
            "inbox_items": inbox["items"],
            "inbox_open": inbox["open"],
        },
        promoted,
    )


def summarize_samples(paths, problems):
    rows = []
    for path in paths:
        try:
            lines = open(path, encoding="utf-8").read().splitlines()
        except (OSError, UnicodeError) as e:
            raise ReadError(f"cannot read {path}: {e}") from e
        for i, line in enumerate(lines, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
                rows.append((row["at"], int(row["rss_kb"]), row.get("script_tick_wait"), row.get("pid")))
            except (ValueError, KeyError, TypeError) as e:
                problems.append(f"{path}:{i}: bad sample ({e})")
    if not rows:
        return None
    rows.sort(key=lambda r: r[0])
    rss = [r[1] for r in rows]
    ticks = [r[2] for r in rows if r[2]]
    tick = None
    if ticks:
        tick = {
            "samples": len(ticks),
            "p95_us_max": max(t["p95_us"] for t in ticks),
            "p50_us_last": ticks[-1]["p50_us"],
            "p95_us_last": ticks[-1]["p95_us"],
            "max_us_max": max(t["max_us"] for t in ticks),
            "tick_budget_max": max(t["tick_budget"] for t in ticks),
            "budget_us": ticks[-1]["budget_us"],
        }
    return {
        "samples": len(rows),
        "first_at": rows[0][0],
        "last_at": rows[-1][0],
        "pids": sorted({str(r[3]) for r in rows}),
        "rss_kb": {"first": rss[0], "last": rss[-1], "max": max(rss), "growth": rss[-1] - rss[0]},
        "tick_wait": tick,
    }


def summarize_tick_log(paths):
    """daemon の log（人が journalctl で file に落とす）の `slow tick` と `script_tick_wait` の行を数える。"""
    if not paths:
        return None
    out = {"lines": 0, "slow_ticks": 0, "script_tick_wait_lines": 0, "tick_budget_warnings": 0}
    for path in paths:
        try:
            lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
        except OSError as e:
            raise ReadError(f"cannot read {path}: {e}") from e
        for line in lines:
            out["lines"] += 1
            if "slow tick" in line and "slow tick phases" not in line:
                out["slow_ticks"] += 1
            if "script_tick_wait" in line:
                out["script_tick_wait_lines"] += 1
                if "WARN" in line:
                    out["tick_budget_warnings"] += 1
    return out


def cmd_report(a):
    problems = []
    missing = missing_tables(a.db)
    if missing:
        print(json.dumps({"schema": "celeris.script-compare-report/1", "tables_missing": missing}, ensure_ascii=False))
        return 3
    rows = read_snapshot(a.db, a.upto_event_id, a.upto_log_id)
    bounded = a.upto_event_id is not None or a.upto_log_id is not None
    modes = rows.get("mode", [])
    mode_of = {m["domain"]: m for m in modes}
    guard = {(r["domain"], r["generation"]): r for r in rows.get("guard", [])}
    rollback, promoted = rollback_summary(rows)
    groups = aggregate(rows.get("record", []), problems)
    domains = []
    for key in sorted(groups):
        domains.append(finish_group(groups[key], guard.get(key), mode_of.get(key[0]), promoted, bounded, problems))
    if not bounded:
        # 記録の無い generation に計数だけがあれば、それも合わない（件数を計数だけで補った DB を見逃さない）。
        for (domain, generation), row in sorted(guard.items()):
            if domain != "*" and (domain, generation) not in groups and row["calls"] != 0:
                problems.append(f"{domain} {generation}: script_guard_state has {row['calls']} calls but no records")
    halted = [r for r in guard.values() if r["domain"] == "*" and r["halted"]]
    report = {
        "schema": "celeris.script-compare-report/1",
        "bounds": rows["bounds"][0],
        "modes": modes,
        "domains": domains,
        "rollback": rollback,
        "guard_halted": [{"reason": r["halt_reason"], "updated_at": r["updated_at"]} for r in halted],
        "shadow_outbox_pending": rows["outbox"][0]["pending"],
        "samples": summarize_samples(a.samples, problems),
        "tick_log": summarize_tick_log(a.tick_log),
        "problems": problems,
    }
    if a.format == "table":
        print_table(report)
    else:
        print(json.dumps(report, ensure_ascii=False, indent=1, sort_keys=True))
    if problems:
        return 1
    if a.require_ready:
        ready = {d["domain"] for d in domains if d["ready"]}
        if not all(x in ready for x in a.require_ready.split(",")):
            return 4
    return 0


def print_table(r):
    print(f"bounds {r['bounds']}")
    for m in r["modes"]:
        print(f"mode {m['domain']} = {m['mode']} generation={m['generation']} version={m['version']} by={m['updated_by']}")
    head = "domain generation match mismatch ts_error invalid_output unavailable rate p50_us p95_us max_us streak/req ready"
    print(head)
    for d in r["domains"]:
        o, lat = d["outcomes"], d["latency"] or {"p50_us": "-", "p95_us": "-", "max_us": "-"}
        print(
            d["domain"], d["generation"], o["match"], o["mismatch"], o["ts_error"], o["invalid_output"], o["unavailable"],
            d["match_rate"], lat["p50_us"], lat["p95_us"], lat["max_us"], f"{d['effective_clean_streak']}/{d['required']}", d["ready"],
        )
        for b in d["ready_blockers"]:
            print(f"  not ready: {b}")
    rb = r["rollback"]
    print(f"rollback to_rust_by_actor={rb['to_rust_by_actor']} automatic={len(rb['automatic_rollbacks'])} "
          f"generation_events={len(rb['generation_events'])} inbox_open={rb['inbox_open']}/{rb['inbox_items']}")
    print(f"guard_halted={r['guard_halted']} shadow_outbox_pending={r['shadow_outbox_pending']}")
    print(f"samples={json.dumps(r['samples'], ensure_ascii=False)}")
    print(f"tick_log={json.dumps(r['tick_log'], ensure_ascii=False)}")
    for p in r["problems"]:
        print(f"PROBLEM {p}")


# --- sample ---------------------------------------------------------------------------------------------------------


def read_proc_status(proc_root, pid):
    fields = {}
    try:
        for line in open(os.path.join(proc_root, str(pid), "status"), encoding="utf-8"):
            k, _, v = line.partition(":")
            fields[k] = v.strip()
    except OSError as e:
        raise ReadError(f"cannot read status of pid {pid}: {e}") from e
    if "VmRSS" not in fields:
        raise ReadError(f"pid {pid} has no VmRSS")
    return fields


def read_daemon_view(a):
    if a.daemon_json:
        try:
            return json.load(open(a.daemon_json, encoding="utf-8"))
        except (OSError, ValueError) as e:
            raise ReadError(f"cannot read {a.daemon_json}: {e}") from e
    token = open(a.token_file, encoding="utf-8").read().strip()
    req = urllib.request.Request(f"{a.api.rstrip('/')}/api/v1/daemon", headers={"Authorization": f"Bearer {token}"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(req, timeout=15) as resp:
            return json.load(resp)
    except (urllib.error.URLError, ValueError, OSError) as e:
        raise ReadError(f"GET /api/v1/daemon failed: {e}") from e


def cmd_sample(a):
    if not a.daemon_json and not (a.api and a.token_file):
        print("sample: --daemon-json か --api と --token-file を渡す", file=sys.stderr)
        return 2
    for i in range(a.count):
        if i:
            time.sleep(a.interval_secs)
        status = read_proc_status(a.proc_root, a.pid)
        view = read_daemon_view(a)
        snap = view.get("snapshot") or {}
        print(
            json.dumps(
                {
                    "schema": "celeris.script-compare-sample/1",
                    "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                    "pid": a.pid,
                    "rss_kb": int(status["VmRSS"].split()[0]),
                    "hwm_kb": int(status.get("VmHWM", "0 kB").split()[0]),
                    "threads": int(status.get("Threads", "0")),
                    "script_tick_wait": snap.get("script_tick_wait"),
                },
                ensure_ascii=False,
                sort_keys=True,
            ),
            flush=True,
        )
    return 0


# --- preflight ------------------------------------------------------------------------------------------------------


def load_toml(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def db_path_of(cfg, base):
    db = cfg.get("db")
    path = db.get("path") if isinstance(db, dict) else db
    if not path:
        return None
    path = os.path.expanduser(str(path))
    return os.path.join(base, path) if not os.path.isabs(path) else path


def listen_of(cfg):
    return (cfg.get("api") or {}).get("listen")


def same_file(a, b):
    try:
        return os.path.samefile(a, b)
    except OSError:
        return os.path.realpath(a) == os.path.realpath(b)


def under(path, root):
    path, root = os.path.realpath(path), os.path.realpath(root)
    return path == root or path.startswith(root.rstrip("/") + "/")


def running_daemons(proc_root):
    """`celeris` の daemon（argv[0] の basename が celeris で --config を持つ process）。bwrap の包みは数えない。"""
    found = []
    try:
        pids = [p for p in os.listdir(proc_root) if p.isdigit()]
    except OSError as e:
        raise ReadError(f"cannot list {proc_root}: {e}") from e
    for pid in sorted(pids, key=int):
        try:
            raw = open(os.path.join(proc_root, pid, "cmdline"), "rb").read()
        except OSError:
            continue
        argv = [x.decode("utf-8", "replace") for x in raw.split(b"\0") if x]
        if not argv or os.path.basename(argv[0]) != "celeris" or "--config" not in argv:
            continue
        i = argv.index("--config")
        release = argv[argv.index("--release") + 1] if "--release" in argv[:-1] else None
        found.append({"pid": int(pid), "exe": argv[0], "config": argv[i + 1] if i + 1 < len(argv) else "", "release": release})
    return found


def measure_configs(roots):
    found = []
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(root):
            depth = os.path.relpath(dirpath, root).count(os.sep) + (dirpath != root)
            if depth >= 3:
                dirnames[:] = []
            if "daemon.toml" in filenames:
                found.append(os.path.join(dirpath, "daemon.toml"))
            dirnames[:] = [d for d in dirnames if d not in ("workspaces", "ws", "runs", "out", "kb", "target", ".git")]
    return sorted(found)


def preflight_isolation(a, cfg, cfg_raw, target_db, target_listen, daemons, check):
    # 測定の config が指してはいけない本番の口と置き場（API・llm proxy・script の state・本番の release）。
    script = cfg.get("script") or {}
    target_refs = [x for x in (target_listen, (cfg.get("llm_proxy") or {}).get("listen"), script.get("state_dir")) if x]
    target_refs.append(os.path.join(a.state_dir, "releases"))
    state_dir = script.get("state_dir")
    leaked = [r for r in a.measure_root if os.path.realpath(r) in cfg_raw or r in cfg_raw]
    check("prod_config_has_no_measure_path", "fail" if leaked else "pass", f"参照している測定の dir: {leaked}")
    if state_dir:
        inside = [r for r in a.measure_root if under(os.path.expanduser(str(state_dir)), r)]
        check("script_state_dir_outside_measure", "fail" if inside else "pass", f"[script] state_dir={state_dir}")
    configs = measure_configs(a.measure_root)
    check("measure_configs_found", "pass" if configs or not a.measure_root else "human", f"{len(configs)} 件")
    bad, seen, stopped_refs = [], [], []
    for path in configs:
        try:
            raw = open(path, encoding="utf-8").read()
            c = tomllib.loads(raw)
        except (OSError, ValueError) as e:
            bad.append(f"{path}: 読めない ({e})")
            continue
        db = db_path_of(c, os.path.dirname(path))
        seen.append({"config": path, "db": db, "listen": listen_of(c), "script_section": "script" in c})
        if "script" in c:
            bad.append(f"{path}: [script] がある")
        if db is None or same_file(db, target_db):
            bad.append(f"{path}: DB が本番と同じか無い ({db})")
        if target_db in raw:
            bad.append(f"{path}: 本番 DB の path を含む")
        for ref in target_refs:
            if ref in raw:
                if any(same_file(d["config"], path) for d in daemons):
                    bad.append(f"{path}: 動いている測定の daemon が本番の {ref} を参照する")
                else:
                    stopped_refs.append(f"{path}: {ref}")
        if listen_of(c) == target_listen:
            bad.append(f"{path}: api listen が本番と同じ ({target_listen})")
    check("measure_configs_separate", "fail" if bad else "pass", "; ".join(bad) or f"{len(seen)} 件すべて DB・listen が別で [script] 無し")
    # 止まった測定の config の参照は今の測定を壊さないが、起動し直すと本番を読むので人に見せる。
    check("stopped_measure_configs_refs", "human" if stopped_refs else "pass", "; ".join(stopped_refs) or "無し")
    # 動いている daemon（本番以外）。
    bad = []
    others = [d for d in daemons if not same_file(d["config"], a.config)]
    for d in others:
        if under(d["exe"], a.state_dir):
            is_measure = any(under(d["config"], r) for r in a.measure_root)
            if is_measure:
                bad.append(f"pid {d['pid']}: 測定の daemon が本番の release の binary で動く ({d['exe']})")
        try:
            c = load_toml(d["config"])
        except FileNotFoundError:
            # 走査の間に終わった一時の daemon（試験）。測定の config が消えていれば破れとして扱う。
            if any(under(d["config"], r) for r in a.measure_root):
                bad.append(f"pid {d['pid']}: 測定の config が無い ({d['config']})")
            continue
        except (OSError, ValueError) as e:
            bad.append(f"pid {d['pid']}: config を読めない ({e})")
            continue
        db = db_path_of(c, os.path.dirname(d["config"]))
        if db is None or same_file(db, target_db):
            bad.append(f"pid {d['pid']}: 本番と同じ DB ({db})")
        if listen_of(c) == target_listen:
            bad.append(f"pid {d['pid']}: 本番と同じ listen ({target_listen})")
        if "script" in c and any(under(d["config"], r) for r in a.measure_root):
            bad.append(f"pid {d['pid']}: 測定の daemon に [script] がある")
    summary = [{k: d[k] for k in ("pid", "config", "release")} for d in others]
    check("other_daemons_separate", "fail" if bad else "pass", "; ".join(bad) or json.dumps(summary, ensure_ascii=False))
    # 測定の binary の置き場は本番の release の symlink でない（promote の prune で消えない実体）。
    bad = [b for b in a.measure_bin if under(b, a.state_dir)]
    for root in a.measure_bin:
        if os.path.isdir(root):
            for name in sorted(os.listdir(root)):
                if under(os.path.join(root, name), a.state_dir):
                    bad.append(os.path.join(root, name))
    check("measure_bin_is_real_copy", "fail" if bad else "pass", "; ".join(bad) or f"{a.measure_bin} は本番の state の外")
    return seen


def preflight_release(a, target, check):
    release = target["release"] if target else None
    gate_path = os.path.join(a.state_dir, "releases", str(release), "gate.json")
    if not release or not os.path.exists(gate_path):
        check("release_gate_has_script_steps", "human", f"gate.json が無い: {gate_path}")
        return
    try:
        gate = json.load(open(gate_path, encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise ReadError(f"cannot read {gate_path}: {e}") from e
    steps = {s.get("step"): s for s in gate.get("steps", [])}
    lacking = [s for s in GATE_STEPS if s not in steps or steps[s].get("exit") != 0 or steps[s].get("skipped")]
    bundle = os.path.join(a.state_dir, "releases", release, "script-bundle")
    has_bundle = os.path.isdir(bundle)
    check(
        "release_gate_has_script_steps",
        "human" if lacking or not has_bundle else "pass",
        f"release {release}: 足りない段 {lacking}、script-bundle/ {'あり' if has_bundle else '無し'}",
    )
    if a.repo:
        sha = gate.get("sha") or release
        p = subprocess.run(
            ["git", "-C", a.repo, "show", f"{sha}:crates/celerisctl/src/commands/script.rs"],
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
        )
        ok = p.returncode == 0 and "GuardCommand" in p.stdout
        check("release_has_ops_cli", "pass" if ok else "human", f"{sha[:12]} の celerisctl に generations・guard の CLI が {'ある' if ok else '無い'}")


def preflight_db(a, db, check):
    version = int(query(db, "SELECT coalesce(max(version),0) FROM schema_migrations")[0])
    check("db_schema_version", "pass" if version >= MIN_SCHEMA else "human", f"schema {version}（要 {MIN_SCHEMA} 以上。起動の migration で当たる）")
    baseline = {"schema_version": version, "events_max_id": int(query(db, "SELECT coalesce(max(id),0) FROM events")[0])}
    missing = missing_tables(db)
    if missing:
        check("db_script_tables", "human", f"無い表: {missing}")
        return baseline
    check("db_script_tables", "pass", "全部ある")
    modes = query_json(db, "SELECT json_object('domain',domain,'mode',mode,'generation',generation_id,'version',version) FROM script_domain_modes ORDER BY domain")
    baseline["modes"] = modes
    baseline["script_compare_log_max_id"] = int(query(db, "SELECT coalesce(max(id),0) FROM script_compare_log")[0])
    moved = [m for m in modes if m["mode"] != "rust"]
    check("db_modes_all_rust", "fail" if moved else "pass", json.dumps(modes, ensure_ascii=False))
    halted = query(db, "SELECT coalesce(sum(halted),0) FROM script_guard_state WHERE domain='*'")[0]
    check("db_guard_not_halted", "fail" if int(halted) else "pass", f"halted={halted}")
    open_items = int(query(db, "SELECT count(*) FROM script_inbox_items WHERE resolved_at IS NULL")[0])
    check("db_no_open_script_inbox", "human" if open_items else "pass", f"未処理 {open_items} 件")
    return baseline


def days_between(first, last):
    parse = lambda t: calendar.timegm(time.strptime(t[:19], "%Y-%m-%dT%H:%M:%S"))
    return (parse(last) - parse(first)) / 86400


def estimate_volume(db, since):
    """本番の実績から件数の見込みを出す（見込みであり、件数に足さない）。lane_rules の記録は `decide_lane` の 1 回ごとに
    作られるが、同じ task の直前の記録と入力が同じなら追記されない（S2-4）。`routing_decided` の features で同じ抑えを当てて数える。"""
    if not since:
        return None
    if not re.fullmatch(r"[0-9TZ:.+-]+", since):
        raise ReadError(f"bad --estimate-since: {since}")
    rows = query_json(
        db,
        f"""SELECT json_object('task',task_id,'ts',ts,'f',json_extract(json,'$.record.decision.features')) FROM events
        WHERE json_extract(json,'$.type')='routing_decided' AND ts>='{since}'
        AND json_extract(json,'$.record.decision.source') IN ('hint','default') ORDER BY id""",
    )
    last, records, tasks = {}, 0, set()
    for r in rows:
        tasks.add(r["task"])
        if last.get(r["task"]) != r["f"]:
            records += 1
        last[r["task"]] = r["f"]
    digests = query_json(
        db,
        f"""SELECT json_object('n',count(*),'first',min(created_at),'last',max(created_at)) FROM chat_messages
        WHERE client_message_id LIKE 'cos-triage-digest:%' AND created_at>='{since}'""",
    )[0]
    days = days_between(rows[0]["ts"], rows[-1]["ts"]) if len(rows) > 1 else 0
    cos_days = days_between(digests["first"], digests["last"]) if digests["n"] > 1 else 0
    per_day = records / days if days else None
    cos_per_day = digests["n"] / cos_days if cos_days else None
    return {
        "since": since,
        "lane_rules_policy_runs": len(rows),
        "lane_rules_distinct_tasks": len(tasks),
        "lane_rules_records_after_dedup": records,
        "lane_rules_span_days": round(days, 2),
        "lane_rules_records_per_day": round(per_day, 2) if per_day else None,
        "lane_rules_days_to_required": round(REQUIRED["lane_rules"] / per_day, 1) if per_day else None,
        "cos_digest_messages": digests["n"],
        "cos_digest_span_days": round(cos_days, 2),
        "cos_digest_per_day": round(cos_per_day, 2) if cos_per_day else None,
        "cos_digest_days_to_required": round(REQUIRED["cos_digest"] / cos_per_day, 1) if cos_per_day else None,
    }


def cmd_preflight(a):
    checks = []

    def check(cid, status, detail):
        checks.append({"id": cid, "status": status, "detail": detail})

    try:
        cfg_raw = open(a.config, encoding="utf-8").read()
        cfg = tomllib.loads(cfg_raw)
    except (OSError, ValueError) as e:
        raise ReadError(f"cannot read config {a.config}: {e}") from e
    base = os.path.dirname(os.path.abspath(a.config))
    db = db_path_of(cfg, base)
    if not db:
        raise ReadError("config has no [db] path")
    script = cfg.get("script")
    if script is None:
        check("config_script_section", "human", "[script] が無い。TS 層は起きない（本番は rust のまま）")
    elif "active_bundle_sha256" in script:
        check("config_script_section", "fail", "消した欄 active_bundle_sha256 がある（新しい binary は読み込みエラー）")
    else:
        sd = os.path.expanduser(str(script.get("state_dir", "")))
        bundles = os.path.join(sd, "script-bundles")
        have = sorted(os.listdir(bundles)) if os.path.isdir(bundles) else []
        check("config_script_section", "pass", f"state_dir={sd} 配備済みの bundle {len(have)} 件")
    daemons = running_daemons(a.proc_root)
    targets = [d for d in daemons if same_file(d["config"], a.config)]
    target = targets[0] if len(targets) == 1 else None
    check(
        "target_daemon_running",
        "pass" if target else "fail",
        json.dumps(targets, ensure_ascii=False) if targets else "この config の daemon が見つからない",
    )
    preflight_release(a, target, check)
    baseline = preflight_db(a, db, check)
    if target:
        # compare の前の RSS（compare の間の推移と比べる基準）。読めなくても preflight は止めない。
        try:
            status = read_proc_status(a.proc_root, target["pid"])
            baseline["daemon_rss_kb"] = int(status["VmRSS"].split()[0])
        except ReadError:
            baseline["daemon_rss_kb"] = None
    measures = preflight_isolation(a, cfg, cfg_raw, db, listen_of(cfg), daemons, check)
    out = {
        "schema": "celeris.script-compare-preflight/1",
        "config": a.config,
        "db": db,
        "listen": listen_of(cfg),
        "release": target["release"] if target else None,
        "baseline": baseline,
        "estimate": estimate_volume(db, a.estimate_since),
        "measure_configs": measures,
        "checks": checks,
    }
    print(json.dumps(out, ensure_ascii=False, indent=1, sort_keys=True))
    statuses = {c["status"] for c in checks}
    return 1 if "fail" in statuses else 3 if "human" in statuses else 0


def main(argv):
    p = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("report")
    r.add_argument("--db", required=True)
    r.add_argument("--samples", action="append", default=[])
    r.add_argument("--tick-log", action="append", default=[])
    r.add_argument("--upto-event-id", type=int)
    r.add_argument("--upto-log-id", type=int)
    r.add_argument("--require-ready")
    r.add_argument("--format", choices=("json", "table"), default="json")
    s = sub.add_parser("sample")
    s.add_argument("--pid", type=int, required=True)
    s.add_argument("--daemon-json")
    s.add_argument("--api")
    s.add_argument("--token-file")
    s.add_argument("--count", type=int, default=1)
    s.add_argument("--interval-secs", type=float, default=600)
    s.add_argument("--proc-root", default="/proc")
    f = sub.add_parser("preflight")
    f.add_argument("--config", required=True)
    f.add_argument("--state-dir", required=True)
    f.add_argument("--measure-root", action="append", default=[])
    f.add_argument("--measure-bin", action="append", default=[])
    f.add_argument("--proc-root", default="/proc")
    f.add_argument("--repo")
    f.add_argument("--estimate-since")
    a = p.parse_args(argv)
    try:
        return {"report": cmd_report, "sample": cmd_sample, "preflight": cmd_preflight}[a.cmd](a)
    except ReadError as e:
        print(f"script-compare: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
