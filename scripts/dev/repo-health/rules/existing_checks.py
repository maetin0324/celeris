"""既存検査の包み（ADR 2026-11-07-repo-health-contract D5、ADR 2026-11-07-repo-health-celeris-profile D2）。

rule `doc.links`・`adr.numbers`・`progress.index`・`human-text`・`source-size` は、profile が書いた command を
head の tree で走らせ、非 0 の exit を finding に写す。どの script を走らせるかは profile にだけ書く
（検査器のコードは repo 固有の script 名を持たない）:

    [rules."doc.links"]
    commands = [["sh", "scripts/dev/check-doc-links.sh", "--root", "{root}"],
                ["sh", "scripts/dev/check-doc-links.sh", "--self-test", "--root", "{root}"]]

- 置換: `{root}` は head の tree の dir（worktree なら repo、commit なら $TMPDIR の展開先）、`{base}` は base の sha。
  root・base は snapshot が 1 か所で決め、script は自分で解決しない。
- commit の head では、展開先を独立した Git の作業ツリーにする（object は alternates で対象 repo を読み、branch の
  ref を写す。HEAD は head の sha）。対象 repo の index・ref・config には書かない。command には GIT_* の環境変数を
  渡さない（command の中の試験が作る一時 repo が対象 repo を書き換えないように）。
- command が無ければ not_applicable（no_command）。起動の前に、argv[0] が PATH に無ければ unsupported（tool_missing）、
  command の script（`_script_of`）が head の tree に file として無ければ unsupported（command_missing）にする
  （shell ごとの exit の違い、dash の 2 に依らない）。起動した後の exit 126・127 も unsupported（command_missing）。
  signal で終わったら error（crashed:<rule>:CommandKilled。検査の結果ではない）。非 0 の exit は fail（finding 1 件。
  出力の `path:line:` の行を evidence にする。無ければ command の script、script が無ければ argv[0]）。
- head だけで評価する（既存検査は今も「全部通る」が合格の条件なので、base に同じ失敗があっても new として数える）。
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess

from health.gitio import git_text
from health.rule import Hit, NotApplicable, Rule, Unsupported


class CommandKilled(RuntimeError):
    """包んだ検査が signal で終わった。検査の結果ではなく基盤の故障なので finding にせず error にする。"""


# script を引数に取る interpreter（名前、script を取らない option の文字、次の引数を値に取る option の文字）。
# ADR 2026-11-11-health-checker-gaps G2。
_INTERPRETERS = (
    (re.compile(r"sh|bash|dash"), "c", "o"),
    (re.compile(r"python(?:3(?:\.\d+)?)?"), "cm", "WX"),
)


def _script_of(argv: list[str]) -> str | None:
    """command が走らせる script の path。無ければ None。

    argv[0]（の basename）が `_INTERPRETERS` の interpreter のときの、option の後の最初の引数だけを script とする。
    `-c`（文字列）・`-m`（module）・`-`（標準入力）は script を取らない。`--` の次の引数は script。
    ほかの tool の引数（例 `-s scripts/tests`）は script とみなさない。
    """
    spec = next(((none, value) for name, none, value in _INTERPRETERS
                 if name.fullmatch(os.path.basename(argv[0]))), None)
    if spec is None:
        return None
    no_script, takes_value = spec
    args = iter(argv[1:])
    for a in args:
        if a == "--":
            return next(args, None)
        if a == "-":
            return None
        if a.startswith("--"):  # 長い option。値を取るのは python の --check-hash-based-pycs だけ
            if a == "--check-hash-based-pycs":
                next(args, None)
            continue
        if a[0] in "-+":  # 短い option の束（例 -Im、-eo pipefail）
            for i, ch in enumerate(a[1:], 1):
                if ch in no_script:
                    return None
                if ch in takes_value:
                    if i == len(a) - 1:
                        next(args, None)
                    break
            continue
        return a
    return None


def _preflight(argv: list[str], root: str) -> str | None:
    """起動の前の確かめ。argv[0] が PATH に無い・script が head の tree に無ければ Unsupported。script を返す。"""
    if shutil.which(argv[0]) is None:
        raise Unsupported(f"tool_missing:{argv[0]}")
    script = _script_of(argv)
    if script is not None and not os.path.isfile(os.path.join(root, script)):
        raise Unsupported(f"command_missing: script {script} が head の tree に無い")
    return script


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


def _clean_env() -> dict:
    """command に渡す環境。GIT_* は渡さない（command の中の試験が作る一時 repo が対象 repo を指さないように）。"""
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.pop("CELERIS_WU_BASE", None)  # base は {base} で渡す。script が環境から別の base を拾わない
    return env


def _git_checkout(ctx, tree, env: dict) -> None:
    """commit の head の展開先を独立した Git の作業ツリーにする（対象 repo の index・ref・config には書かない）。

    object は alternates で対象 repo のものを読み、branch の ref は写す（main との merge-base を引けるように）。
    HEAD は head の sha（detached）、index は head の tree。
    """
    root = tree.checkout_dir()
    if os.path.exists(os.path.join(root, ".git")):
        return
    src = ctx.snapshot.root
    common = git_text(src, "rev-parse", "--path-format=absolute", "--git-common-dir")

    def g(*args, stdin=None):
        subprocess.run(["git", "-C", root, *args], check=True, env=env, input=stdin, capture_output=True,
                       text=True)

    g("init", "-q")
    with open(os.path.join(root, ".git", "objects", "info", "alternates"), "w", encoding="utf-8") as fh:
        fh.write(os.path.join(common, "objects") + "\n")
    refs = git_text(src, "for-each-ref", "--format=%(objectname) %(refname)", "refs/heads/")
    g("update-ref", "--stdin", stdin="".join(f"update {r} {o}\n" for o, r in
                                          (line.split(" ", 1) for line in refs.splitlines() if line)))
    g("update-ref", "--no-deref", "HEAD", ctx.snapshot.head_sha)
    g("read-tree", "HEAD")
    subprocess.run(["git", "-C", root, "update-index", "-q", "--refresh"], env=env, capture_output=True)


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
        env = _clean_env()
        if not tree.worktree:
            _git_checkout(ctx, tree, env)
        for argv in commands:
            argv = [subs.get(a, a) for a in argv]
            script = _preflight(argv, root)
            proc = subprocess.run(argv, cwd=root, env=env, capture_output=True, text=True, timeout=900)
            shown = " ".join(a if a not in (root,) else "{root}" for a in argv)
            if proc.returncode in (126, 127):
                raise Unsupported(f"command_missing: {shown} が起動できない（exit {proc.returncode}）")
            if proc.returncode < 0:
                raise CommandKilled(f"{shown} が signal {-proc.returncode} で終わった")
            if proc.returncode == 0:
                continue
            script = script or argv[0]
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
