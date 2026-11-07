"""言語 adapter: file を言語と種類に分ける（拡張子・shebang・先頭行の生成 marker）。

言語:
- adapter がある言語: rust・python・shell・typescript（.js 系も含む）・markdown
- adapter が無いコードの言語（go・java・c など）: `ADAPTERLESS_CODE` に挙げる。構造 rule の対象 diff に
  これが入れば、その rule は unsupported（`adapter_missing:<lang>`）になる（engine）。
- コードでない file: data（設定・lock・画像など）と other（判定できない）

種類（class）: markdown は常に docs。他は generated（先頭 10 行の marker か path）> fixture > migration > test > docs > product の順で最初に当たったもの。
profile の `[classes]` に path の正規表現を足せる（既定に加わる。base 側の profile の値）。
"""

from __future__ import annotations

import re
from dataclasses import dataclass

ADAPTER_LANGUAGES = ("rust", "python", "shell", "typescript", "markdown")
CLASSES = ("generated", "fixture", "migration", "test", "docs", "product")

_EXT = {
    ".rs": "rust",
    ".py": "python",
    ".pyi": "python",
    ".sh": "shell",
    ".bash": "shell",
    ".zsh": "shell",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".mts": "typescript",
    ".cts": "typescript",
    ".js": "typescript",
    ".jsx": "typescript",
    ".mjs": "typescript",
    ".cjs": "typescript",
    ".md": "markdown",
    ".markdown": "markdown",
}
ADAPTERLESS_CODE = {
    ".go": "go", ".java": "java", ".kt": "kotlin", ".kts": "kotlin", ".scala": "scala",
    ".c": "c", ".h": "c", ".cc": "cpp", ".cpp": "cpp", ".cxx": "cpp", ".hpp": "cpp", ".hh": "cpp",
    ".cu": "cuda", ".f90": "fortran", ".f": "fortran", ".rb": "ruby", ".php": "php", ".swift": "swift",
    ".cs": "csharp", ".lua": "lua", ".zig": "zig", ".hs": "haskell", ".ml": "ocaml", ".ex": "elixir",
    ".exs": "elixir", ".erl": "erlang", ".jl": "julia", ".r": "r", ".pl": "perl", ".dart": "dart",
    ".vue": "vue", ".svelte": "svelte",
}
_DATA_EXT = {
    ".json", ".jsonl", ".toml", ".yaml", ".yml", ".lock", ".txt", ".csv", ".tsv", ".svg", ".png",
    ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".pdf", ".html", ".css", ".scss", ".xml", ".ini", ".cfg",
    ".conf", ".sql", ".env", ".gitignore", ".gitattributes", ".service", ".timer", ".socket", ".woff",
    ".woff2", ".ttf", ".wasm", ".bin", ".gz", ".zip", ".tar", ".patch", ".diff", ".snap",
}
_SHEBANG = (
    (re.compile(r"python"), "python"),
    (re.compile(r"\b(ba|z|da)?sh\b"), "shell"),
    (re.compile(r"\b(node|deno|bun)\b"), "typescript"),
)
GENERATED_MARKERS = ("@generated", "DO NOT EDIT", "Code generated", "This file was automatically generated")

# 既定の種類の path 規則（言語を問わない一般の慣習）。profile の [classes] がこれに加わる。
DEFAULT_CLASS_PATHS = {
    "generated": [r"(^|/)generated/", r"\.gen\.[a-z]+$", r"(^|/)__generated__/"],
    "fixture": [r"(^|/)(fixtures?|testdata|__snapshots__|__fixtures__)/", r"(^|/)[^/]*_fixture\.[a-z]+$"],
    "migration": [r"(^|/)migrations?/"],
    "test": [
        r"(^|/)(tests?|e2e|benches|__tests__|spec)/",
        r"(^|/)(tests|[^/]*_tests?)\.rs$",
        r"\.(test|spec)\.(ts|tsx|mts|js|jsx|mjs|cjs)$",
        r"(^|/)test_[^/]*\.py$",
        r"(^|/)[^/]*_test\.(py|go|sh)$",
    ],
    "docs": [r"(^|/)docs?/"],
}


@dataclass(frozen=True)
class FileInfo:
    path: str
    language: str  # adapter 言語・ADAPTERLESS_CODE の値・"data"・"other"
    cls: str  # CLASSES のどれか
    is_code: bool  # data・other・markdown 以外

    @property
    def has_adapter(self) -> bool:
        return self.language in ADAPTER_LANGUAGES


def _ext(path: str) -> str:
    name = path.rsplit("/", 1)[-1]
    if name.startswith(".") and name.count(".") == 1:
        return name
    return "." + name.rsplit(".", 1)[-1].lower() if "." in name else ""


def language_of(path: str, head: bytes | None) -> str:
    ext = _ext(path)
    if ext in _EXT:
        return _EXT[ext]
    if ext in ADAPTERLESS_CODE:
        return ADAPTERLESS_CODE[ext]
    if ext in _DATA_EXT:
        return "data"
    if head and head.startswith(b"#!"):
        line = head.split(b"\n", 1)[0].decode("utf-8", "replace")
        for pat, lang in _SHEBANG:
            if pat.search(line):
                return lang
    return "other"


class Classifier:
    """path の種類を決める。正規表現は既定 + profile の [classes]（base 側）。"""

    def __init__(self, extra: dict[str, list[str]] | None = None) -> None:
        self.paths: dict[str, list[re.Pattern]] = {}
        for cls in CLASSES[:-1]:
            pats = list(DEFAULT_CLASS_PATHS.get(cls, [])) + list((extra or {}).get(cls, []))
            self.paths[cls] = [re.compile(p) for p in pats]

    def classify(self, path: str, head: bytes | None) -> FileInfo:
        lang = language_of(path, head)
        cls = "product"
        text_head = (head or b"")[:2048].decode("utf-8", "replace")
        first_lines = "\n".join(text_head.splitlines()[:10])
        if lang == "markdown":
            cls = "docs"
        elif any(m in first_lines for m in GENERATED_MARKERS) and lang != "data":
            cls = "generated"
        else:
            for c in CLASSES[:-1]:
                if any(p.search(path) for p in self.paths[c]):
                    cls = c
                    break
        is_code = lang not in ("data", "other", "markdown")
        return FileInfo(path=path, language=lang, cls=cls, is_code=is_code)
