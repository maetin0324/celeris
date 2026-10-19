#!/usr/bin/env python3
"""Jev 副比較の独立読取元（測定 daemon の SQLite）から原記録を ID で読む reader（標準ライブラリのみ）。

信頼境界:
  - 接続先は運用者が入力とは独立に設定した環境変数 CELERIS_JEV_SOURCE_DB だけ。evidence 内の
    source / path / command、CELERIS_DB / CELERIS_RUN_DB、既定の DB には接続しない（実行もしない）。
  - 開き方は SQLite URI `mode=ro` + `PRAGMA query_only=ON`。読み取りは 1 本の読取 transaction の中で
    行い、全ての表を同じ snapshot から読む。元 DB の内容・schema は変えない。
  - schema は task-core の SCHEMA_VERSION（schema_migrations の最大 version）と必要な表・欄を照合する。
    未知の版・欠けた表/欄は診断を返し、live 合格の材料にしない（推測で埋めない）。

CLI: `python3 scripts/dev/jev_source_reader.py --campaign <id> [--evidence-dir <dir>] [--out <file>]`
終了コード: 0 読取成功 / 2 信頼元なし・到達不能・evidence 内の自称 source / 3 未知 schema・必要欄なし・
campaign 不在（live 不可）。
"""
import argparse
import json
import os
import sqlite3
import sys
import urllib.parse

ENV_SOURCE = "CELERIS_JEV_SOURCE_DB"
# 接続先に使わない（運用者の設定と取り違えない）daemon / worker の DB 変数。
FORBIDDEN_ENV = ("CELERIS_DB", "CELERIS_RUN_DB")
# crates/task-core/src/store/migrations.rs の SCHEMA_VERSION。上げたら必要欄も見直す。
KNOWN_SCHEMA_VERSIONS = (86,)
REQUIRED = {
    "schema_migrations": ["version"],
    "tasks": ["id", "parent_id", "status", "kind", "created_at", "json"],
    "runs": ["run_id", "task_id", "work_unit_id", "role", "seq", "status", "model",
             "usage_json", "metrics_json", "started_at", "finished_at"],
    "events": ["id", "task_id", "seq", "ts", "json"],
    "task_campaigns": ["task_id", "campaign_id", "case_id", "arm", "repetition",
                       "manifest_hash", "routing_setting", "created_at"],
    "task_routing_overrides": ["task_id", "override_json", "created_at"],
    "decision_log_runs": ["run_id", "decision_id", "campaign_id", "arm", "task_id",
                          "work_unit_id", "parent_run_id", "attempt_no", "routing_mode",
                          "proposed_first_model", "exec_model", "provider_model_version",
                          "fallback_reason", "classifier_fallback", "run_end", "manifest_hash"],
    "decision_log_usage": ["dedup_key", "run_id", "decision_id", "campaign_id", "task_id",
                           "role", "is_retry", "list_cost_usd", "cash_cost_usd",
                           "attribution_rule", "shared_overhead"],
    "decision_log_interventions": ["intervention_id", "kind", "task_id", "campaign_id", "at", "wall_ms",
                                   "source_event"],
    "decision_log_campaign_activity": ["campaign_id", "last_activity_at"],
    "route_decisions": ["decision_id", "run_id", "input_json", "output_json", "judge_json",
                        "stage", "created_at"],
}
EXIT_OK, EXIT_NO_SOURCE, EXIT_NOT_LIVE = 0, 2, 3


class SourceError(Exception):
    """信頼元が使えない（exit 2）か live 不可（exit 3）。diagnostics は機械向けの診断の列。"""

    def __init__(self, exit_code, diagnostics):
        self.exit_code = exit_code
        self.diagnostics = diagnostics
        super().__init__("; ".join(f"{d['code']}: {d['detail']}" for d in diagnostics))


def diag(code, detail):
    return {"code": code, "detail": detail}


def _real(path):
    return os.path.realpath(os.path.abspath(path))


def _inside(path, directory):
    d = _real(directory)
    return path == d or path.startswith(d.rstrip(os.sep) + os.sep)


def configured_source(environ=None, evidence_dir=None):
    """運用者が設定した読取元の実 path を返す。引数・evidence からは path を受け取らない。"""
    env = os.environ if environ is None else environ
    raw = env.get(ENV_SOURCE, "")
    if not raw.strip():
        raise SourceError(EXIT_NO_SOURCE, [diag(
            "no_trusted_source",
            f"{ENV_SOURCE} が未設定。運用者が測定 daemon の DB を明示しない限り読まない（既定 DB へ暗黙に接続しない）")])
    if not os.path.isabs(raw):
        raise SourceError(EXIT_NO_SOURCE, [diag("source_not_absolute", f"{ENV_SOURCE} は絶対 path にする: {raw}")])
    path = _real(raw)
    for name in FORBIDDEN_ENV:
        other = env.get(name, "")
        if other.strip() and _real(other) == path:
            raise SourceError(EXIT_NO_SOURCE, [diag(
                "source_is_daemon_db", f"{ENV_SOURCE} が {name} と同じ DB を指す（daemon/worker の DB は信頼元にしない）")])
    if evidence_dir is not None and _inside(path, evidence_dir):
        raise SourceError(EXIT_NO_SOURCE, [diag(
            "source_in_evidence_dir", "読取元が evidence dir の中にある（入力に同梱した自称原記録は信頼元にしない）")])
    if not os.path.isfile(path):
        raise SourceError(EXIT_NO_SOURCE, [diag("source_unreachable", f"読取元の file が無い: {path}")])
    return path


def check_schema(conn):
    """schema の診断の列（空なら必要な表・欄が揃っている）と版数を返す。"""
    out = []
    tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    version = None
    if "schema_migrations" in tables:
        version = conn.execute("SELECT COALESCE(MAX(version), 0) FROM schema_migrations").fetchone()[0]
        if version not in KNOWN_SCHEMA_VERSIONS:
            out.append(diag("unknown_schema", f"schema_migrations の版 {version} は既知の {list(KNOWN_SCHEMA_VERSIONS)} でない"))
    for table, cols in REQUIRED.items():
        if table not in tables:
            out.append(diag("missing_table", table))
            continue
        have = {r[1] for r in conn.execute(f"PRAGMA table_info({table})")}
        for c in cols:
            if c not in have:
                out.append(diag("missing_column", f"{table}.{c}"))
    return version, out


def _rows(cur):
    names = [d[0] for d in cur.description]
    return [dict(zip(names, r)) for r in cur.fetchall()]


def _json(text):
    if text is None:
        return None
    try:
        return json.loads(text)
    except ValueError:
        return {"_unparsed": text}


def _chunks(ids, n=500):
    ids = sorted(set(ids))
    for i in range(0, len(ids), n):
        yield ids[i:i + n]


class SourceSnapshot:
    """読取専用・単一 snapshot の接続。`with SourceSnapshot.open(path) as s:` で使う。"""

    def __init__(self, conn, path, version):
        self.conn = conn
        self.path = path
        self.schema_version = version

    @classmethod
    def open(cls, path):
        uri = "file:" + urllib.parse.quote(path) + "?mode=ro"
        conn = None
        try:
            conn = sqlite3.connect(uri, uri=True, isolation_level=None)
            conn.execute("PRAGMA query_only=ON")
            if conn.execute("PRAGMA query_only").fetchone()[0] != 1:
                conn.close()
                raise SourceError(EXIT_NO_SOURCE, [diag("query_only_unavailable", path)])
            # 1 本の読取 transaction。最初の SELECT で snapshot が固定され、以後の読みは全て同じ時点を見る。
            conn.execute("BEGIN")
            conn.execute("SELECT count(*) FROM sqlite_master").fetchone()
            version, problems = check_schema(conn)
        except sqlite3.Error as e:
            if conn is not None:
                conn.close()
            raise SourceError(EXIT_NO_SOURCE, [diag("source_unreachable", f"{path}: {e}")])
        if problems:
            conn.close()
            raise SourceError(EXIT_NOT_LIVE, problems)
        return cls(conn, path, version)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        if self.conn is not None:
            try:
                self.conn.execute("ROLLBACK")
            except sqlite3.Error:
                pass
            self.conn.close()
            self.conn = None

    def _by_ids(self, sql, ids):
        out = []
        for chunk in _chunks(ids):
            q = sql.format(ph=",".join("?" * len(chunk)))
            out.extend(_rows(self.conn.execute(q, chunk)))
        return out

    # --- ID による抽出 ---
    def tasks(self, ids):
        rows = self._by_ids("SELECT id, parent_id, status, kind, created_at, json FROM tasks WHERE id IN ({ph}) ORDER BY id", ids)
        for r in rows:
            r["json"] = _json(r["json"])
        return rows

    def runs(self, task_ids):
        rows = self._by_ids(
            "SELECT run_id, task_id, work_unit_id, role, seq, status, model, usage_json, metrics_json, "
            "started_at, finished_at FROM runs WHERE task_id IN ({ph}) ORDER BY task_id, seq, run_id", task_ids)
        for r in rows:
            r["usage_json"] = _json(r["usage_json"])
            r["metrics_json"] = _json(r["metrics_json"])
        return rows

    def events(self, task_ids):
        rows = self._by_ids("SELECT id, task_id, seq, ts, json FROM events WHERE task_id IN ({ph}) ORDER BY id", task_ids)
        for r in rows:
            r["json"] = _json(r["json"])
            r["type"] = r["json"].get("type") if isinstance(r["json"], dict) else None
        return rows

    def decision_runs(self, campaign_id):
        return _rows(self.conn.execute(
            "SELECT * FROM decision_log_runs WHERE campaign_id = ? ORDER BY run_id", (campaign_id,)))

    def usage(self, campaign_id, run_ids, decision_ids):
        """campaign に直接付いた行と、campaign 欄が NULL で run/decision から辿れる行（export と同じ帰属）。"""
        rows = {r["dedup_key"]: r for r in _rows(self.conn.execute(
            "SELECT * FROM decision_log_usage WHERE campaign_id = ? ORDER BY dedup_key", (campaign_id,)))}
        for r in self._by_ids("SELECT * FROM decision_log_usage WHERE campaign_id IS NULL AND run_id IN ({ph})", run_ids):
            rows.setdefault(r["dedup_key"], r)
        for r in self._by_ids("SELECT * FROM decision_log_usage WHERE campaign_id IS NULL AND decision_id IN ({ph})", decision_ids):
            rows.setdefault(r["dedup_key"], r)
        return [rows[k] for k in sorted(rows)]

    def interventions(self, campaign_id):
        return _rows(self.conn.execute(
            "SELECT * FROM decision_log_interventions WHERE campaign_id = ? ORDER BY intervention_id", (campaign_id,)))

    def campaign_activity(self, campaign_id):
        row = self.conn.execute(
            "SELECT last_activity_at FROM decision_log_campaign_activity WHERE campaign_id = ?", (campaign_id,)).fetchone()
        return row[0] if row else None

    def responses(self, decision_ids):
        rows = self._by_ids(
            "SELECT decision_id, run_id, input_json, output_json, judge_json, stage, created_at "
            "FROM route_decisions WHERE decision_id IN ({ph}) ORDER BY decision_id", decision_ids)
        for r in rows:
            for k in ("input_json", "output_json", "judge_json"):
                r[k] = _json(r[k])
        return rows

    def routing_overrides(self, task_ids):
        rows = self._by_ids("SELECT task_id, override_json, created_at FROM task_routing_overrides WHERE task_id IN ({ph}) ORDER BY task_id", task_ids)
        for r in rows:
            r["override_json"] = _json(r["override_json"])
        return rows

    # --- campaign 所属（task_campaigns の行を持つ最も近い祖先が決める。task-core と同じ規則） ---
    def campaign_tasks(self, campaign_id):
        roots = _rows(self.conn.execute(
            "SELECT * FROM task_campaigns WHERE campaign_id = ? ORDER BY task_id", (campaign_id,)))
        if not roots:
            return [], []
        tree = _rows(self.conn.execute(
            "WITH RECURSIVE sub(id) AS (SELECT task_id FROM task_campaigns WHERE campaign_id = ?1 "
            "UNION SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id) "
            "SELECT t.id, t.parent_id, c.campaign_id AS own_campaign FROM sub JOIN tasks t ON t.id = sub.id "
            "LEFT JOIN task_campaigns c ON c.task_id = t.id", (campaign_id,)))
        parent = {r["id"]: r["parent_id"] for r in tree}
        own = {r["id"]: r["own_campaign"] for r in tree if r["own_campaign"] is not None}
        members = []
        for tid in parent:
            cur = tid
            while cur is not None and cur not in own:
                cur = parent.get(cur)
            if cur is not None and own[cur] == campaign_id:
                members.append(tid)
        return roots, sorted(members)

    def extract(self, campaign_id):
        """campaign の原記録を ID で束ねる（同じ snapshot）。判定はしない。"""
        diags = []
        roots, members = self.campaign_tasks(campaign_id)
        if not roots:
            raise SourceError(EXIT_NOT_LIVE, [diag("campaign_not_found", f"task_campaigns に campaign_id={campaign_id} の行が無い")])
        druns = self.decision_runs(campaign_id)
        run_ids = [r["run_id"] for r in druns]
        decision_ids = [r["decision_id"] for r in druns if r["decision_id"]]
        events = self.events(members)
        hashes = sorted({r["manifest_hash"] for r in roots if r["manifest_hash"]})
        if not hashes:
            diags.append(diag("manifest_hash_missing", "task_campaigns に manifest_hash が無い"))
        elif len(hashes) > 1:
            diags.append(diag("manifest_hash_mixed", f"campaign の root に複数の manifest_hash: {hashes}"))
        diags.append(diag("manifest_body_not_in_source",
                          "DB は manifest の hash だけを持ち本文を持たない（本文の照合は別の原記録が要る）"))
        stray = sorted({r["task_id"] for r in druns if r["task_id"] and r["task_id"] not in set(members)})
        if stray:
            diags.append(diag("decision_run_outside_campaign_tree", f"campaign 木の外の task の run 行: {stray}"))
        return {
            "source": {"env": ENV_SOURCE, "path": self.path, "schema_version": self.schema_version,
                       "open_mode": "uri mode=ro + PRAGMA query_only=ON, single read transaction"},
            "campaign": {"campaign_id": campaign_id, "roots": roots, "manifest_hashes": hashes},
            "tasks": self.tasks(members),
            "routing_overrides": self.routing_overrides(members),
            "runs": self.runs(members),
            "events": events,
            "routing_decided": [e for e in events if e["type"] == "routing_decided"],
            "decision_runs": druns,
            "usage": self.usage(campaign_id, run_ids, decision_ids),
            "interventions": self.interventions(campaign_id),
            "last_activity_at": self.campaign_activity(campaign_id),
            "responses": self.responses(decision_ids),
            "diagnostics": diags,
        }


def read_campaign(campaign_id, environ=None, evidence_dir=None):
    path = configured_source(environ, evidence_dir)
    with SourceSnapshot.open(path) as snap:
        return snap.extract(campaign_id)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--campaign", required=True)
    ap.add_argument("--evidence-dir", help="入力の evidence dir（読取元がこの中なら拒否する。中身は読まない）")
    ap.add_argument("--out", help="抽出結果 JSON の出力先（省略時は標準出力）")
    a = ap.parse_args(argv)
    try:
        data = read_campaign(a.campaign, evidence_dir=a.evidence_dir)
    except SourceError as e:
        print(json.dumps({"ok": False, "exit": e.exit_code, "diagnostics": e.diagnostics}, ensure_ascii=False), file=sys.stderr)
        return e.exit_code
    text = json.dumps(data, ensure_ascii=False, sort_keys=True, indent=1)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write(text + "\n")
    else:
        print(text)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
