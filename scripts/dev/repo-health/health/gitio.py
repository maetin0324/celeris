"""git の呼び出し。外部 tool は git だけを前提にし、無ければ呼び出し側が exit 2 にする。"""

from __future__ import annotations

import subprocess


class GitError(Exception):
    """git が非 0 で終わった。message は git の stderr の先頭行（絶対 path を含みうるので report には入れない）。"""


def git(repo: str, *args: str, check: bool = True, timeout: int = 300) -> bytes:
    proc = subprocess.run(
        ["git", "-C", repo, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    if check and proc.returncode != 0:
        first = proc.stderr.decode("utf-8", "replace").strip().splitlines()
        raise GitError(f"git {args[0]}: {first[0] if first else proc.returncode}")
    return proc.stdout


def git_ok(repo: str, *args: str) -> bool:
    proc = subprocess.run(["git", "-C", repo, *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return proc.returncode == 0


def git_text(repo: str, *args: str) -> str:
    return git(repo, *args).decode("utf-8", "replace").strip()


class BlobReader:
    """`git cat-file --batch` を 1 本だけ起こし、blob を sha で読む。"""

    def __init__(self, repo: str) -> None:
        self.repo = repo
        self._proc: subprocess.Popen | None = None

    def _ensure(self) -> subprocess.Popen:
        if self._proc is None:
            self._proc = subprocess.Popen(
                ["git", "-C", self.repo, "cat-file", "--batch"],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
        return self._proc

    def read(self, sha: str) -> bytes | None:
        proc = self._ensure()
        assert proc.stdin is not None and proc.stdout is not None
        proc.stdin.write(sha.encode() + b"\n")
        proc.stdin.flush()
        header = proc.stdout.readline().decode().split()
        if len(header) < 2 or header[1] == "missing":
            return None
        size = int(header[2])
        data = proc.stdout.read(size)
        proc.stdout.read(1)  # 末尾の改行
        return data

    def close(self) -> None:
        if self._proc is not None:
            assert self._proc.stdin is not None
            self._proc.stdin.close()
            self._proc.wait()
            self._proc = None
