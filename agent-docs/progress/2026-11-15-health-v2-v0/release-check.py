#!/usr/bin/env python3
"""健全性v2 V0: 検証済みの release 候補があるかを読み取りだけで判定する。

使い方:
  python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --releases /local/celeris/state/releases
  python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --self-test

判定（release.md の §1 と同じ）:
  releases dir の直下の各 release `<sha12>/` について
    1. `gate.json` が読めて `ok` が真、`sha`（40 桁）が dir 名で始まる（release.sh の write_gate_json の形）
    2. その sha S が repo にあり、`git diff --name-only S <head>` の path がすべて `agent-docs/progress/` の下
       （候補の後に進捗の文書しか変わっていない = 同じコードを gate と verify で確かめた）
    3. `verify.json`（verify.sh が `<release>/verify.json` に書く。欄 sha12・at・ok・live_ok・checks）が読めて
       `ok` が真、`sha12` が dir 名と一致
  をすべて満たす release を探す。複数あれば verify.json の `at` が新しいものを選ぶ。
  見つかれば stdout に sha12 を 1 行出して exit 0。無ければ release ごとの理由を stderr に出して exit 1。
  引数・repo の誤りは exit 2。

  gate が落ちた sha は release として公開されず、gate.json は `<releases>/.build/<sha12>/gate.json` に残る
  （release.sh）。<head> の sha12 にそれがあれば、落ちた段（failed_step）を理由に添える。

この script は何も書かない（releases dir・本番 DB・本番 host に触れない）。`--self-test` だけが $TMPDIR の下に
一時の releases dir と git repo を作り、終わったら消す。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

ALLOWED_PREFIX = "agent-docs/progress/"


@dataclass
class Verdict:
    sha12: str
    ok: bool
    reasons: list[str] = field(default_factory=list)
    verify_at: str = ""
    live_ok: object = None


def _read_json(path: Path) -> tuple[dict | None, str]:
    if not path.is_file():
        return None, f"{path.name} が無い"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return None, f"{path.name} を読めない（{exc}）"
    if not isinstance(data, dict):
        return None, f"{path.name} が JSON の object でない"
    return data, ""


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )


def resolve_head(repo: Path, head: str) -> str:
    cp = _git(repo, "rev-parse", "--verify", f"{head}^{{commit}}")
    if cp.returncode != 0:
        raise ValueError(f"{repo} で {head} を commit に解決できない: {cp.stderr.strip()}")
    return cp.stdout.strip()


def check_release(rel: Path, repo: Path, head_sha: str) -> Verdict:
    v = Verdict(sha12=rel.name, ok=False)
    gate, err = _read_json(rel / "gate.json")
    if gate is None:
        v.reasons.append(f"gate: {err}")
        return v
    if gate.get("ok") is not True:
        v.reasons.append(f"gate: ok={gate.get('ok')!r}（failed_step={gate.get('failed_step') or '-'}）")
        return v
    sha = str(gate.get("sha") or "")
    if len(sha) != 40 or not sha.startswith(rel.name):
        v.reasons.append(f"gate: sha={sha!r} が dir 名 {rel.name} と合わない")
        return v
    if _git(repo, "cat-file", "-e", f"{sha}^{{commit}}").returncode != 0:
        v.reasons.append(f"git: {sha[:12]} がこの repo に無い")
        return v
    diff = _git(repo, "diff", "--name-only", sha, head_sha)
    if diff.returncode != 0:
        v.reasons.append(f"git: diff が失敗した（{diff.stderr.strip()}）")
        return v
    outside = [p for p in diff.stdout.splitlines() if p and not p.startswith(ALLOWED_PREFIX)]
    if outside:
        shown = ", ".join(outside[:3]) + (" …" if len(outside) > 3 else "")
        v.reasons.append(
            f"diff: 候補の後に {ALLOWED_PREFIX} の外の {len(outside)} path が変わった（{shown}）"
        )
        return v
    verify, err = _read_json(rel / "verify.json")
    if verify is None:
        v.reasons.append(f"verify: {err}（verify.sh {rel.name} を実行していない）")
        return v
    if verify.get("sha12") != rel.name:
        v.reasons.append(f"verify: sha12={verify.get('sha12')!r} が dir 名と合わない")
        return v
    if verify.get("ok") is not True:
        failed = [
            str(c.get("name"))
            for c in verify.get("checks") or []
            if isinstance(c, dict) and c.get("ok") is not True
        ]
        v.reasons.append(f"verify: ok={verify.get('ok')!r}（落ちた検査: {', '.join(failed) or '-'}）")
        return v
    v.ok = True
    v.verify_at = str(verify.get("at") or "")
    v.live_ok = verify.get("live_ok")
    return v


def failed_gate_note(releases: Path, head_sha: str) -> str:
    gate, _ = _read_json(releases / ".build" / head_sha[:12] / "gate.json")
    if gate is None or gate.get("ok") is True:
        return ""
    return (
        f"<head> {head_sha[:12]} の gate は落ちている（.build/{head_sha[:12]}/gate.json の "
        f"failed_step={gate.get('failed_step') or '-'}）"
    )


def run_check(releases: Path, repo: Path, head: str) -> int:
    if not releases.is_dir():
        print(f"release-check: releases dir が無い: {releases}", file=sys.stderr)
        return 2
    try:
        head_sha = resolve_head(repo, head)
    except ValueError as exc:
        print(f"release-check: {exc}", file=sys.stderr)
        return 2
    rels = sorted(p for p in releases.iterdir() if p.is_dir() and not p.name.startswith("."))
    verdicts = [check_release(p, repo, head_sha) for p in rels]
    passed = sorted((v for v in verdicts if v.ok), key=lambda v: (v.verify_at, v.sha12))
    if passed:
        best = passed[-1]
        print(best.sha12)
        print(
            f"release-check: {best.sha12} は gate ok・verify ok（at={best.verify_at}、live_ok={best.live_ok}）、"
            f"<head> {head_sha[:12]} との差は {ALLOWED_PREFIX} の下だけ",
            file=sys.stderr,
        )
        return 0
    print(
        f"release-check: <head> {head_sha[:12]} に合う検証済みの release が {releases} に無い",
        file=sys.stderr,
    )
    if not verdicts:
        print("  release が 1 つも無い", file=sys.stderr)
    for v in verdicts:
        print(f"  {v.sha12}: {'; '.join(v.reasons)}", file=sys.stderr)
    note = failed_gate_note(releases, head_sha)
    if note:
        print(f"  {note}", file=sys.stderr)
    return 1


# ---- self-test -----------------------------------------------------------------------------


def _st_git(repo: Path, *args: str) -> str:
    cp = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=release-check",
            "-c",
            "user.email=release-check@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            *args,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=True,
    )
    return cp.stdout.strip()


def _st_commit(repo: Path, path: str, text: str, msg: str) -> str:
    f = repo / path
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(text, encoding="utf-8")
    _st_git(repo, "add", path)
    _st_git(repo, "commit", "-q", "-m", msg)
    return _st_git(repo, "rev-parse", "HEAD")


def _st_release(releases: Path, sha: str, gate_ok: bool, verify_ok: bool | None) -> None:
    rel = releases / sha[:12]
    rel.mkdir(parents=True)
    steps = [{"step": "cargo-test", "exit": 0 if gate_ok else 101, "secs": 1.0}]
    (rel / "gate.json").write_text(
        json.dumps(
            {
                "sha": sha,
                "sha12": sha[:12],
                "ok": gate_ok,
                "failed_step": "" if gate_ok else "cargo-test",
                "steps": steps,
            }
        ),
        encoding="utf-8",
    )
    if verify_ok is not None:
        checks = [
            {"id": "1", "name": "start-and-migrate", "ok": True},
            {"id": "6", "name": "smoke", "ok": verify_ok},
        ]
        (rel / "verify.json").write_text(
            json.dumps(
                {
                    "sha12": sha[:12],
                    "at": "2026-11-16T00:00:00Z",
                    "ok": verify_ok,
                    "live_ok": True,
                    "checks": checks,
                }
            ),
            encoding="utf-8",
        )


def _st_run(releases: Path, repo: Path, head: str) -> tuple[int, str, str]:
    cp = subprocess.run(
        [sys.executable, "-I", str(Path(__file__).resolve()), "--releases", str(releases), "--repo", str(repo), "--head", head],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    return cp.returncode, cp.stdout, cp.stderr


def self_test() -> int:
    root = Path(tempfile.mkdtemp(prefix="release-check-", dir=os.environ.get("TMPDIR") or None))
    failures: list[str] = []
    try:
        repo = root / "repo"
        repo.mkdir()
        _st_git(repo, "init", "-q", "-b", "main")
        cand = _st_commit(repo, "src/lib.rs", "pub fn f() {}\n", "candidate")
        docs_head = _st_commit(repo, "agent-docs/progress/x.md", "# x\n", "progress only")
        _st_git(repo, "branch", "docs-head", docs_head)
        code_head = _st_commit(repo, "src/lib.rs", "pub fn f() { g() }\n", "code after candidate")
        _st_git(repo, "branch", "code-head", code_head)
        # 偽の .build（gate の落ちた sha の置き場）。releases として数えられないことも確かめる。
        cases = [
            # (名前, gate ok, verify ok, head, 期待 exit, stdout に含む, stderr に含む)
            ("合格", True, True, "docs-head", 0, cand[:12], "verify ok"),
            ("gate 失敗", False, True, "docs-head", 1, "", "gate: ok=False"),
            ("verify 失敗", True, False, "docs-head", 1, "", "verify: ok=False（落ちた検査: smoke）"),
            ("候補の後のコード差分", True, True, "code-head", 1, "", "src/lib.rs"),
        ]
        for name, gate_ok, verify_ok, head, want_rc, want_out, want_err in cases:
            releases = root / f"releases-{len(failures)}-{name.replace(' ', '_')}"
            releases.mkdir()
            (releases / ".build" / "000000000000").mkdir(parents=True)
            _st_release(releases, cand, gate_ok, verify_ok)
            before = sorted(str(p.relative_to(releases)) for p in releases.rglob("*"))
            rc, out, err = _st_run(releases, repo, head)
            after = sorted(str(p.relative_to(releases)) for p in releases.rglob("*"))
            ok = rc == want_rc and want_out in out and want_err in err and before == after
            if want_rc != 0 and out.strip():
                ok = False
            print(f"self-test {'ok  ' if ok else 'FAIL'} {name}: exit={rc} stdout={out.strip()!r}")
            if not ok:
                failures.append(name)
                print(f"  stderr: {err.strip()}")
            shutil.rmtree(releases)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    if failures:
        print(f"self-test: {len(failures)} 例が落ちた: {', '.join(failures)}")
        return 1
    print("self-test: 4 例すべて期待どおり")
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--releases", type=Path, help="release の置き場（本番は /local/celeris/state/releases）")
    ap.add_argument("--repo", type=Path, default=Path("."), help="git repo（既定 カレントディレクトリ）")
    ap.add_argument("--head", default="HEAD", help="比べる commit（既定 HEAD）")
    ap.add_argument("--self-test", action="store_true", help="$TMPDIR に偽の releases と git repo を作り 4 例を確かめる")
    args = ap.parse_args(argv)
    if args.self_test:
        return self_test()
    if args.releases is None:
        ap.error("--releases か --self-test が要る")
    return run_check(args.releases, args.repo, args.head)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
