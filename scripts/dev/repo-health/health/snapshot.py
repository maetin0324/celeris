"""snapshot の解決（ADR D1・D7）: base・head の sha、dirty、shallow、submodule、変更 file と変更行。

- 1 回の実行は 1 つの Git 作業ツリー。submodule（gitlink）は両側の sha を記録するだけで中に入らない。
- base が解決できず repo が shallow なら base は不達（reachable=false）。shallow でなければ使い方誤り。
- head は commit（ref・sha）か `worktree`（未 commit の変更と ignore されない未追跡 file を含む）。
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import tempfile
from dataclasses import dataclass, field

from .gitio import BlobReader, GitError, git, git_ok, git_text

WORKTREE = "worktree"


class UsageError(Exception):
    """report を書かずに exit 2 にする入力の誤り。message の先頭は機械で読める語。"""


@dataclass
class Tree:
    """片側（base か head）の追跡 file の一覧と読み出し。submodule は含まない。"""

    side: str  # "base" | "head"
    repo: str
    blobs: dict[str, str | None]  # path -> blob sha（worktree の file は None で disk から読む）
    reader: BlobReader
    worktree: bool = False
    _cache: dict[str, bytes | None] = field(default_factory=dict)
    infos: dict = field(default_factory=dict)  # engine の分類の cache（path -> FileInfo）
    _checkout: str | None = None

    def files(self) -> list[str]:
        return sorted(self.blobs)

    def has(self, path: str) -> bool:
        return path in self.blobs

    def read_bytes(self, path: str) -> bytes | None:
        if path not in self.blobs:
            return None
        if path not in self._cache:
            if self.worktree:
                try:
                    with open(os.path.join(self.repo, path), "rb") as fh:
                        self._cache[path] = fh.read()
                except OSError:
                    self._cache[path] = None
            else:
                sha = self.blobs[path]
                self._cache[path] = self.reader.read(sha) if sha else None
        return self._cache[path]

    def read_text(self, path: str) -> str | None:
        data = self.read_bytes(path)
        if data is None or b"\0" in data[:8192]:
            return None
        return data.decode("utf-8", "replace")

    def blob_sha(self, path: str) -> str | None:
        """policy.files の blob。worktree は内容から git の blob sha を計算する。"""
        if path not in self.blobs:
            return None
        if self.blobs[path]:
            return self.blobs[path]
        data = self.read_bytes(path)
        if data is None:
            return None
        return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()

    def checkout_dir(self) -> str:
        """外部 tool（cargo など）に渡す展開先。worktree はそのまま、commit は $TMPDIR に展開する。"""
        if self.worktree:
            return self.repo
        if self._checkout is None:
            dest = tempfile.mkdtemp(prefix=f"repo-health-{self.side}-")
            for path in self.files():
                data = self.read_bytes(path)
                if data is None:
                    continue
                full = os.path.join(dest, path)
                os.makedirs(os.path.dirname(full), exist_ok=True)
                with open(full, "wb") as fh:
                    fh.write(data)
            self._checkout = dest
        return self._checkout

    def cleanup(self) -> None:
        if self._checkout and not self.worktree:
            shutil.rmtree(self._checkout, ignore_errors=True)
            self._checkout = None


@dataclass
class Snapshot:
    root: str
    base_input: str
    head_input: str
    base_sha: str | None
    base_reachable: bool
    head_kind: str  # commit | worktree
    head_sha: str
    dirty: bool
    shallow: bool
    submodules: list[dict]
    changed: list[str]  # base..head の変更 path（submodule は除く）。不達なら空
    changed_lines: dict[str, list[tuple[int, int]]]  # head 側の追加・変更行の範囲（両端含む）
    base_tree: Tree | None
    head_tree: Tree

    def finding_snapshot(self) -> dict:
        return {"base": self.base_sha, "head": self.head_sha, "dirty": self.dirty, "shallow": self.shallow}

    def report_snapshot(self) -> dict:
        return {
            "base": {"sha": self.base_sha, "reachable": self.base_reachable},
            "head": {"kind": self.head_kind, "sha": self.head_sha, "dirty": self.dirty},
            "shallow": self.shallow,
            "submodules": self.submodules,
        }


def find_root(path: str) -> str:
    if not os.path.isdir(path):
        raise UsageError(f"not_a_git_repo: {path} は dir でない")
    try:
        root = git_text(path, "rev-parse", "--show-toplevel")
    except GitError:
        raise UsageError(f"not_a_git_repo: {path} は Git の作業ツリーでない") from None
    if not root:
        raise UsageError(f"not_a_git_repo: {path} は bare か作業ツリーの外")
    return root


def _resolve_commit(root: str, rev: str) -> str | None:
    if not git_ok(root, "rev-parse", "--verify", "-q", f"{rev}^{{commit}}"):
        return None
    sha = git_text(root, "rev-parse", "--verify", "-q", f"{rev}^{{commit}}")
    if not git_ok(root, "cat-file", "-e", f"{sha}^{{tree}}"):
        return None
    return sha


def _ls_tree(root: str, sha: str) -> tuple[dict[str, str], dict[str, str]]:
    """(file の path -> blob sha, submodule の path -> commit sha)"""
    files: dict[str, str] = {}
    subs: dict[str, str] = {}
    out = git(root, "ls-tree", "-r", "-z", "--full-tree", sha)
    for entry in out.split(b"\0"):
        if not entry:
            continue
        meta, path = entry.split(b"\t", 1)
        mode, kind, obj = meta.decode().split()
        p = path.decode("utf-8", "surrogateescape")
        if kind == "commit":
            subs[p] = obj
        elif kind == "blob" and mode != "120000":
            files[p] = obj
    return files, subs


def _worktree_files(root: str) -> tuple[dict[str, None], dict[str, str]]:
    files: dict[str, None] = {}
    subs: dict[str, str] = {}
    staged = git(root, "ls-files", "-z", "--stage")
    for entry in staged.split(b"\0"):
        if not entry:
            continue
        meta, path = entry.split(b"\t", 1)
        mode, obj, _stage = meta.decode().split()
        if mode == "160000":
            subs[path.decode("utf-8", "surrogateescape")] = obj
    out = git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    for raw in out.split(b"\0"):
        if not raw:
            continue
        p = raw.decode("utf-8", "surrogateescape")
        full = os.path.join(root, p)
        if p in subs or os.path.islink(full) or not os.path.isfile(full):
            continue
        files[p] = None
    return files, subs


_HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def _changed_lines(diff: bytes) -> dict[str, list[tuple[int, int]]]:
    lines: dict[str, list[tuple[int, int]]] = {}
    current: str | None = None
    for raw in diff.split(b"\n"):
        if raw.startswith(b"+++ "):
            target = raw[4:].decode("utf-8", "surrogateescape")
            current = target[2:] if target.startswith("b/") else None
            continue
        if current is None or not raw.startswith(b"@@"):
            continue
        m = _HUNK.match(raw.decode("utf-8", "replace"))
        if not m:
            continue
        start, count = int(m.group(1)), int(m.group(2) or "1")
        if count:
            lines.setdefault(current, []).append((start, start + count - 1))
    return lines


def resolve(repo: str, base: str, head: str) -> Snapshot:
    root = find_root(repo)
    shallow = git_text(root, "rev-parse", "--is-shallow-repository") == "true"
    reader = BlobReader(root)

    if head == WORKTREE:
        head_sha = _resolve_commit(root, "HEAD")
        if head_sha is None:
            raise UsageError("head_not_found: 作業ツリーの HEAD に commit が無い")
        head_files, head_subs = _worktree_files(root)
        status = git(root, "status", "--porcelain", "-z", "--untracked-files=all", "--ignore-submodules=none")
        dirty = bool(status.strip(b"\0"))
        head_tree = Tree("head", root, dict(head_files), reader, worktree=True)
        kind = "worktree"
    else:
        head_sha = _resolve_commit(root, head)
        if head_sha is None:
            raise UsageError(f"head_not_found: --head {head} が commit に解決できない")
        files, head_subs = _ls_tree(root, head_sha)
        head_tree = Tree("head", root, dict(files), reader)
        dirty = False
        kind = "commit"

    base_sha = _resolve_commit(root, base)
    if base_sha is None and not shallow:
        raise UsageError(f"base_not_found: --base {base} が commit に解決できない")
    reachable = base_sha is not None
    base_tree: Tree | None = None
    base_subs: dict[str, str] = {}
    changed: list[str] = []
    changed_lines: dict[str, list[tuple[int, int]]] = {}
    if reachable:
        assert base_sha is not None
        files, base_subs = _ls_tree(root, base_sha)
        base_tree = Tree("base", root, dict(files), reader)
        diff_args = [base_sha] if kind == "worktree" else [base_sha, head_sha]
        names = git(root, "diff", "--no-renames", "--name-only", "-z", "--no-ext-diff", *diff_args)
        changed_set = {n.decode("utf-8", "surrogateescape") for n in names.split(b"\0") if n}
        if kind == "worktree":
            untracked = git(root, "ls-files", "-z", "--others", "--exclude-standard")
            for raw in untracked.split(b"\0"):
                p = raw.decode("utf-8", "surrogateescape")
                if p and p in head_tree.blobs:
                    changed_set.add(p)
                    text = head_tree.read_text(p)
                    n = len(text.splitlines()) if text else 0
                    if n:
                        changed_lines[p] = [(1, n)]
        sub_paths = set(base_subs) | set(head_subs)
        changed = sorted(p for p in changed_set if p not in sub_paths)
        diff = git(root, "diff", "--no-renames", "-U0", "--no-color", "--no-ext-diff", *diff_args)
        for p, ranges in _changed_lines(diff).items():
            if p not in sub_paths:
                changed_lines.setdefault(p, []).extend(ranges)
    submodules = [
        {"path": p, "base_sha": base_subs.get(p), "head_sha": head_subs.get(p)}
        for p in sorted(set(base_subs) | set(head_subs))
    ]
    return Snapshot(
        root=root,
        base_input=base,
        head_input=head,
        base_sha=base_sha,
        base_reachable=reachable,
        head_kind=kind,
        head_sha=head_sha,
        dirty=dirty,
        shallow=shallow,
        submodules=submodules,
        changed=changed,
        changed_lines={p: sorted(r) for p, r in sorted(changed_lines.items())},
        base_tree=base_tree,
        head_tree=head_tree,
    )
