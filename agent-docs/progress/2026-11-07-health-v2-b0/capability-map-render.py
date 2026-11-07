#!/usr/bin/env python3
"""Render capability-map.json as the human-readable capability-map.md.

Run from the repo root:
  python3 agent-docs/progress/2026-11-07-health-v2-b0/capability-map-render.py <out.md>
"""
import collections
import json
import subprocess
import sys

BASE = "agent-docs/progress/2026-11-07-health-v2-b0"

KIND_JA = {
    "fail_closed": "fail-closed",
    "atomic_state_event": "state+event の原子性",
    "credential_sandbox": "credential/sandbox",
    "human_authority": "人の権限",
    "restart_replay": "restart/replay",
    "lease_exclusion": "lease/排他",
    "budget": "budget",
    "reviewed_candidate_identity": "reviewed candidate の同一性",
    "other": "その他",
}
DIFF_JA = {
    "missing_row": "行が無い",
    "missing_test": "試験が無い",
    "wrong_owner": "owner が違う",
    "missing_invariant": "不変条件が無い",
    "stale": "古い記述",
    "wrong_path": "path が違う",
}


def cell(s):
    return str(s).replace("|", "\\|").replace("\n", " ")


def short(items, n):
    head = items[:n]
    rest = len(items) - len(head)
    return "<br>".join(head) + (f"<br>ほか {rest} 件" if rest > 0 else "")


def owners_cell(c):
    by = collections.OrderedDict()
    for o in c["owners"]:
        by.setdefault(o["crate"], [])
        m = o["module"]
        if m not in by[o["crate"]]:
            by[o["crate"]].append(m)
    return "<br>".join(f"{k}: {', '.join(v)}" for k, v in by.items())


def inv_cell(c):
    kinds = collections.Counter(i["kind"] for i in c["invariants"])
    label = "、".join(f"{KIND_JA.get(k, k)} {n}" for k, n in kinds.most_common())
    first = c["invariants"][0]["text"] if c["invariants"] else ""
    return f"{label}<br>例: {first}"


def cost_cell(c):
    costs, none = {}, 0
    for t in c["test_costs"]:
        if t["build_link_secs"] is None:
            none += 1
        else:
            costs[t["target"].split(" ")[0]] = t["build_link_secs"]
    parts = [f"{k} {v:.1f}" for k, v in sorted(costs.items(), key=lambda kv: -kv[1])]
    if none:
        parts.append(f"未計測 {none} 件")
    return "<br>".join(parts) or "なし"


def main(out):
    d = json.load(open(f"{BASE}/capability-map.json"))
    s = d["summary"]
    base = d["base_sha"]
    arch = subprocess.run(
        ["git", "show", f"{base}:docs/architecture-map.md"],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()

    def arch_subsystem(n):
        line = arch[n - 1] if 0 < n <= len(arch) else ""
        if line.startswith("|"):
            return line.split("|")[1].strip()
        return line.lstrip("# ").strip()[:60]

    caps = d["capabilities"]
    multi = d["multi_writer_state"]
    mk = collections.Counter(m["kind"] for m in multi)
    cross = [m for m in multi if len(m["writer_crates"]) >= 2]
    diffs = d["arch_map_diffs"]
    dk = collections.Counter(a["kind"] for a in diffs)

    L = []
    w = L.append
    w("# 健全性v2 B0: capability ごとの owner・不変条件・public port・試験と、共有状態の書き手")
    w("")
    w("---")
    w("tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]")
    w("---")
    w("")
    w("## 結論")
    w("")
    w(f"- 主要な capability {s['capabilities']} 個について、owner（crate/module）・不変条件・public port・"
      f"関連試験（{s['tests']} 件、全 path 実在）・試験コストを表 1 にまとめた。"
      f"試験コストが未計測の試験は {s['tests_without_cost']} 件ある（段 1 で build を測ったのは "
      "task-core・task-dispatch・task-api・task-worker・celeris の 5 crate だけ）。")
    w(f"- 書き手が複数ある共有状態は {s['multi_writer_state']} 件（SQLite 表 {mk['sqlite_table']}・"
      f"event kind {mk['event_kind']}・file {mk['file']}）。そのうち {len(cross)} 件は複数の crate が書く。")
    w("- crate の normal 依存に循環は無い。module 単位の循環は 3 か所（task-core の 10 module、"
      "task-api の middleware↔state、task-api の 9 module）。")
    w(f"- docs/architecture-map.md との食い違いは {s['arch_map_diffs']} 件"
      f"（{'、'.join(f'{DIFF_JA.get(k, k)} {n}' for k, n in dk.most_common())}）。地図はここでは直していない。")
    w("- 人が決めること: 統合設計（g1-v0-design）で、(1) task-api が直接書く表（cos_inbox_items・"
      "chat_client_requests・browser_task_policies）を task-core の store 関数へ寄せるか、"
      "(2) 複数 crate が組み立てる event kind（transitioned など）の書き手を store 関数に絞るか、"
      "(3) 表 4 の食い違いを architecture-map にいつ反映するか、を決める。")
    w("")
    w(f"基準 commit は着手時の `git merge-base HEAD main`（SHA は capability-map.json の base_sha）。"
      f"機械可読の全データは repo の `{BASE}/capability-map.json`"
      "（schema celeris.health-v2-b0.capability-map/1）にある。")
    w("")

    w("## 1. capability 表")
    w("")
    w("不変条件の欄は種類ごとの件数と 1 件目の本文。public port と関連試験は最初の 3 件を示し、残りは件数だけ書く。"
      "試験コストは段 1 の test-targets.json の build_no_run（crate ごとの `cargo nextest run --no-run` の秒数）で、"
      "同じ crate の試験は同じ値を共有する。")
    w("")
    w("| capability | owner | 不変条件 | public port | 関連試験 | 試験コスト秒 |")
    w("|---|---|---|---|---|---|")
    for c in caps:
        ports = [f"{p['kind']}: `{p['name']}`" for p in c["public_ports"]]
        tests = [f"`{t}`" for t in c["tests"]]
        w("| " + " | ".join(cell(x) for x in [
            f"{c['id']}<br>{c['name']}", owners_cell(c), inv_cell(c),
            short(ports, 3), short(tests, 3), cost_cell(c),
        ]) + " |")
    w("")
    inv = collections.Counter(i["kind"] for c in caps for i in c["invariants"])
    w("不変条件の種類の合計: " + "、".join(f"{KIND_JA.get(k, k)} {n}" for k, n in inv.most_common()) + "。")
    w("")

    w("## 2. crate 依存と module の use の向き")
    w("")
    w("crate 依存は `cargo metadata --no-deps` の workspace member 間の依存（dev は括弧書き）。")
    w("")
    w("| crate | 依存先 |")
    w("|---|---|")
    dd = collections.OrderedDict()
    for e in d["crate_deps"]:
        dd.setdefault(e["from"], []).append(e["to"] + ("" if e["kind"] == "normal" else f"（{e['kind']}）"))
    for k, v in dd.items():
        w(f"| {k} | {', '.join(v)} |")
    w("")
    w("- 依存の向きは一方向で、task-core が最下層、task-ops がその上、task-dispatch・task-api・task-worker が"
      "さらに上、celeris と celerisctl が最上位にある。")
    w("- task-dispatch は task-worker に normal で依存する（use 125 件）。task-api も task-worker を dev で使う。")
    w("")
    w("主要 6 crate の非試験 source の use を、他 crate へ向かうものだけ数えた（use の件数）。")
    w("")
    w("| 使う crate | 使われる crate | use の件数 |")
    w("|---|---|---|")
    x = collections.Counter()
    for u in d["module_uses"]:
        if "::" in u["to_module"]:
            x[(u["crate"], u["to_module"].split("::")[0])] += u["count"]
    for (a, b), n in x.most_common():
        w(f"| {a} | {b} | {n} |")
    w("")
    w("module 単位の循環（crate 内の強連結成分）:")
    w("")
    for cy in d["cycles"]:
        pairs = "、".join("↔".join(p) for p in cy.get("mutual_pairs", []))
        w(f"- {cy['crate']}: {', '.join(cy['modules'])}（相互参照 {pairs}）")
    w("")

    w("## 3. 書き手が複数ある共有状態")
    w("")
    w("書き手が 2 crate 以上、または同じ crate の 2 つ以上の top-level module にまたがるものを挙げる。"
      "crate をまたぐものを先に書く。")
    w("")
    w("| 種類 | 名前 | 書き手の crate | 書き手の module |")
    w("|---|---|---|---|")
    for m in sorted(multi, key=lambda m: (-len(m["writer_crates"]), m["kind"], m["name"])):
        w("| " + " | ".join(cell(v) for v in [
            m["kind"], f"`{m['name']}`", ", ".join(m["writer_crates"]), ", ".join(m["writer_modules"]),
        ]) + " |")
    w("")
    w("- 段 deps-shared-state の数え（表 14・event 20・file 5）より増えたのは、各 capability の調査が"
      "書き手を足した分（表 browser_waits・daemon_instances、event child_task_created など）。")
    w("- 現行の書き手が見つからない event kind が 4 種ある（model_role_assignment_changed・"
      "project_plan_proposed・project_plan_decided・routing_shadow_recorded）。読み取りにだけ現れる。残す意図は未確認。")
    w("")

    w("## 4. docs/architecture-map.md との食い違い")
    w("")
    w("行番号は基準 commit の docs/architecture-map.md の行。")
    w("")
    w("| 行 | 地図の subsystem | 種類 | 観察 |")
    w("|---|---|---|---|")
    for a in diffs:
        w("| " + " | ".join(cell(v) for v in [
            a["arch_map_line"], arch_subsystem(a["arch_map_line"]),
            DIFF_JA.get(a["kind"], a["kind"]), a["observed"],
        ]) + " |")
    w("")

    w("## 計測の方法と限界")
    w("")
    w("- 6 つの調査 unit の断片（capability-map/*.json と module-deps.json）を "
      f"`{BASE}/capability-map-assemble.py` で統合した。同じ id の capability は無かった"
      "（重複したら owner を両方残す規則）。")
    w("- 共有状態の書き手は SQL 文と `Event::X` の組み立ての grep による。動的に組み立てた SQL と、"
      "store 関数の呼び手を辿った書き込みは数えていない（漏れは未確認）。")
    w("- 試験コストは build と link の秒数で、試験の実行時間ではない。task-ops・llm-proxy・campaign・"
      "script-runtime・celerisctl・web・control-ts の試験は未計測。")
    w("")
    open(out, "w", encoding="utf-8").write("\n".join(L))


if __name__ == "__main__":
    main(sys.argv[1])
