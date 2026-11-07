"""既存検査の包み（ADR 2026-11-07-repo-health-contract D5、ADR 2026-11-07-repo-health-celeris-profile D2）。

rule `doc.links`・`adr.numbers`・`progress.index`・`human-text`・`source-size` は、profile が書いた command を
head の tree で走らせ、非 0 の exit を finding に写す。どの script を走らせるかは profile にだけ書く
（検査器のコードは repo 固有の script 名を持たない）:

    [rules."doc.links"]
    commands = [["sh", "scripts/dev/check-doc-links.sh", "--root", "{root}"],
                ["sh", "scripts/dev/check-doc-links.sh", "--self-test", "--root", "{root}"]]

- 置換: `{root}` は head の tree の dir（worktree なら repo、commit なら $TMPDIR の展開先）、`{base}` は base の sha。
  root・base は snapshot が 1 か所で決め、script は自分で解決しない。
- commit の head では、script の git は一時 index（head の tree）と repo の object を見る（GIT_DIR・GIT_WORK_TREE・
  GIT_INDEX_FILE）。repo の index・作業ツリーには書かない。
- command が無ければ not_applicable（no_command）。argv[0] が PATH に無い・script の file が無い（exit 126・127）は
  unsupported。非 0 の exit は fail（finding 1 件。出力の `path:line:` の行を evidence にする。無ければ command の file）。
- head だけで評価する（既存検査は今も「全部通る」が合格の条件なので、base に同じ失敗があっても new として数える）。
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile

from health.gitio import git_text
from health.rule import Hit, NotApplicable, Rule, Unsupported

_LOC = re.compile(r"^\s*(?:-\s+)?([A-Za-z0-9_.][^\s:]*):(\d+):")
_MAX_EVIDENCE = 40


def _commands(ctx) -> list[list[str]]:
    raw = ctx.config.get("commands")
    bad = sorted(set(ctx.config) - {"commands", "enabled", "required"})
    if bad:
        raise ValueError(f"未知の設定 {bad}")
    if not raw:
        raise NotApplicable("no_command: profile の rules.<id>.commands が無い")
    if not isinstance(raw, list) or not all(isinstance(c, list) and c and all(isinstance(a, str) for a in c)
                                            for c in raw):
        raise ValueError("commands は文字列の配列の配列")
    return raw


def _env(ctx, tree, tmp: str) -> dict:
    env = dict(os.environ)
    env.pop("CELERIS_WU_BASE", None)  # base は {base} で渡す。script が環境から別の base を拾わない
    if tree.worktree:
        return env
    root = ctx.snapshot.root
    git_dir = git_text(root, "rev-parse", "--absolute-git-dir")
    index = os.path.join(tmp, "index")
    subprocess.run(["git", "-C", root, "read-tree", ctx.snapshot.head_sha], check=True,
                   env={**env, "GIT_INDEX_FILE": index}, capture_output=True)
    env.update({"GIT_DIR": git_dir, "GIT_WORK_TREE": tree.checkout_dir(), "GIT_INDEX_FILE": index})
    return env


def _evidence(out: str, fallback: str, side: str) -> list[dict]:
    ev = []
    for line in out.splitlines():
        m = _LOC.match(line)
        if m and not m.group(1).startswith(("/", "..")):
            ev.append({"path": m.group(1), "line": int(m.group(2)), "ref": side, "excerpt": line.strip()})
        if len(ev) >= _MAX_EVIDENCE:
            break
    if not ev:
        tail = [x for x in out.splitlines() if x.strip()][-3:]
        ev.append({"path": fallback, "ref": side, "excerpt": " / ".join(tail) or "(出力なし)"})
    return ev


def _rule(rule_id: str, kind: str) -> Rule:
    def evaluate(ctx, tree):
        commands = _commands(ctx)
        root = tree.checkout_dir()
        subs = {"{root}": root, "{base}": ctx.snapshot.base_sha or ""}
        hits = []
        with tempfile.TemporaryDirectory(prefix="repo-health-cmd-") as tmp:
            env = _env(ctx, tree, tmp)
            for argv in commands:
                argv = [subs.get(a, a) for a in argv]
                if shutil.which(argv[0]) is None:
                    raise Unsupported(f"tool_missing:{argv[0]}")
                proc = subprocess.run(argv, cwd=root, env=env, capture_output=True, text=True, timeout=900)
                shown = " ".join(a if a not in (root,) else "{root}" for a in argv)
                if proc.returncode in (126, 127):
                    raise Unsupported(f"command_missing: {shown} が起動できない（exit {proc.returncode}）")
                if proc.returncode == 0:
                    continue
                script = next((a for a in argv[1:] if a.startswith("scripts/") or a.endswith((".sh", ".py"))),
                              argv[0])
                hits.append(Hit(
                    evidence=_evidence(proc.stdout + "\n" + proc.stderr, script, tree.side),
                    scope=("repo", "."),
                    rationale=f"既存検査 {shown} が exit {proc.returncode} で終わった",
                    key=f"command|{shown.replace(ctx.snapshot.base_sha or '{base}', '{base}')}",
                ))
        return hits

    return Rule(id=rule_id, kind=kind, adapter="docs" if kind == "docs" else "generic", evaluate=evaluate,
                head_only=True)


RULES = [
    _rule("doc.links", "docs"),
    _rule("adr.numbers", "docs"),
    _rule("progress.index", "docs"),
    _rule("human-text", "docs"),
    _rule("source-size", "structure"),
]
