#!/usr/bin/env python3
"""Jev 副比較（B1/C1）の「live 承認の証跡」が run/ に揃っているかの実在確認（docs/ops/jev-c1-live-check.md §7）。

本 script は存在と内部の食い違いだけを確かめる。live かどうかを決めるのは operator の承認記録
（operator-approval.md）で、script が live を判定・証明することはない。証跡が 1 つでも欠ければ非 0 で不足一覧を出す。

  jev-live-approval-check.py --run-dir <STATE_DIR>/run --measurement-id <ID>

run-dir の中身（measure-launcher.sh で起こした測定 daemon の、その run の証跡）:
  launcher.meta           $STATE_DIR/launcher.meta の写し（starttime / config / release / exe / exe_sha256）
  config-identity.json    GET /api/v1/config の identity 欄（pid・release）。token・秘密は含めない
  daemon-image.sha256     sha256sum /proc/<pid>/exe の hash（先頭の 64 桁）
  state-dir-db.txt        STATE_DIR の DB の絶対 path（1 行）
  export/                 その run の celerisctl campaign export 一式（records.jsonl summary.json provenance.json routing_events.jsonl）
  operator-approval.md    operator の承認記録（行形式: measurement_id / approved_by / approved_at / decision: live-approved）

終了コード: 0 証跡が揃っている（live の承認そのものは operator の記録の内容で、script は判定しない）/ 1 不足・食い違い。
"""
import argparse
import json
import os
import re
import sys

EXPORT_FILES = ["records.jsonl", "summary.json", "provenance.json", "routing_events.jsonl"]
SHA = re.compile(r"^[0-9a-f]{64}$")


def read(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError:
        return None


def check(run_dir, measurement_id):
    missing = []

    def need(name, why):
        p = os.path.join(run_dir, name)
        text = read(p) if not name.endswith("/") else None
        if name.endswith("/"):
            if not os.path.isdir(p) or not os.listdir(p):
                missing.append(f"{name}: {why}（無い・空）")
            return None
        if text is None or not text.strip():
            missing.append(f"{name}: {why}（無い・空）")
            return None
        return text

    if not os.path.isdir(run_dir):
        return [f"run dir {run_dir} が無い（証跡が 1 つも取得されていない）"]
    meta = need("launcher.meta", "launcher の起動記録")
    if meta is not None:
        kv = dict(l.split("=", 1) for l in meta.splitlines() if "=" in l)
        for k in ("starttime", "config", "release", "exe", "exe_sha256"):
            if not kv.get(k):
                missing.append(f"launcher.meta: {k} が無い")
        meta_sha = kv.get("exe_sha256", "")
    else:
        meta_sha = ""
    ident = need("config-identity.json", "GET /api/v1/config の identity")
    if ident is not None:
        try:
            j = json.loads(ident)
            j = j.get("identity", j)
            for k in ("pid", "release"):
                if j.get(k) in (None, ""):
                    missing.append(f"config-identity.json: {k} が無い")
        except (ValueError, AttributeError):
            missing.append("config-identity.json: JSON として読めない")
    img = need("daemon-image.sha256", "実行像（/proc/<pid>/exe）の sha256")
    if img is not None:
        h = img.split()[0]
        if not SHA.match(h):
            missing.append("daemon-image.sha256: 64 桁の sha256 ではない")
        elif meta_sha and h != meta_sha:
            missing.append("daemon-image.sha256: launcher.meta の exe_sha256 と一致しない")
    db = need("state-dir-db.txt", "STATE_DIR の DB の絶対 path")
    if db is not None and not db.strip().startswith("/"):
        missing.append("state-dir-db.txt: 絶対 path ではない")
    exp = os.path.join(run_dir, "export")
    if not os.path.isdir(exp):
        missing.append("export/: その run の export が無い")
    else:
        for f in EXPORT_FILES:
            t = read(os.path.join(exp, f))
            if t is None or not t.strip():
                missing.append(f"export/{f}: 無い・空")
    ap = need("operator-approval.md", "operator の承認記録")
    if ap is not None:
        kv = {}
        for l in ap.splitlines():
            m = re.match(r"^(measurement_id|approved_by|approved_at|decision):\s*(\S.*)$", l.strip())
            if m:
                kv[m.group(1)] = m.group(2).strip()
        for k in ("measurement_id", "approved_by", "approved_at"):
            if not kv.get(k):
                missing.append(f"operator-approval.md: {k}: の行が無い")
        if kv.get("decision") != "live-approved":
            missing.append("operator-approval.md: decision: live-approved の行が無い（未承認）")
        if kv.get("measurement_id") and kv["measurement_id"] != measurement_id:
            missing.append(f"operator-approval.md: measurement_id が {measurement_id} と一致しない")
    return missing


def main(argv=None):
    ap = argparse.ArgumentParser(description="live 承認の証跡が run/ に揃っているかの実在確認（live の判定はしない）")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--measurement-id", required=True)
    a = ap.parse_args(argv)
    missing = check(a.run_dir, a.measurement_id)
    print(json.dumps({"run_dir": a.run_dir, "measurement_id": a.measurement_id, "missing": missing,
                      "scope": "証跡の実在・内部整合の確認のみ。live かどうかは operator の承認記録で決まる"},
                     ensure_ascii=False, indent=2))
    if missing:
        print(f"証跡不足: {len(missing)} 件（live 未確認）", file=sys.stderr)
        return 1
    print("証跡は揃っている（live の承認は operator の記録の内容で判断する）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
