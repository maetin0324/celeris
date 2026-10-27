#!/usr/bin/env python3
"""Lint the fixed Japanese texts that Celeris shows to people (ADR 2026-10-27-yomiyasu-human-text D4).

The Rust string literals that contain Japanese are taken from the files in FILES (code after the
first `#[cfg(test)]` and `//` comment lines are skipped), one literal per paragraph, and checked with
the vendored `config/skills/yomiyasu/scripts/yomiyasu_lint.py` (Python standard library only, no
network). A warn/error finding fails the check unless `scripts/dev/human-text-lint.allow` lists it
(`rule<TAB>snippet<TAB>reason`). Literals are fragments of format strings, so a finding that only
exists because of the cut (e.g. a label before a list) goes to the allow list with its reason.

  python3 scripts/dev/check-human-text.py              check FILES
  python3 scripts/dev/check-human-text.py --self-test  the checker finds a known bad sample
"""

import json
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
LINT = os.path.join(ROOT, "config", "skills", "yomiyasu", "scripts", "yomiyasu_lint.py")
ALLOW = os.path.join(ROOT, "scripts", "dev", "human-text-lint.allow")

# Fixed texts a person reads: notifications (Discord), the root completion summary, inbox items,
# the notice feed, the matching question, bundled decisions, plan gate, CoS triage, and the
# human-text rules handed to CoS and workers.
FILES = [
    "crates/celeris/src/notify.rs",
    "crates/task-ops/src/root_done.rs",
    "crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs",
    "crates/task-ops/src/human_inbox.rs",
    "crates/task-ops/src/notify_feed.rs",
    "crates/task-ops/src/matching.rs",
    "crates/task-ops/src/plan_gate.rs",
    "crates/task-core/src/decision.rs",
    "crates/task-core/src/chat/triage.rs",
    "crates/task-dispatch/src/dispatcher/cos_chat/triage.rs",
    "crates/task-worker/src/human_text.rs",
]

JAPANESE = re.compile(r"[ぁ-んァ-ヶ一-龥]")
LITERAL = re.compile(r'"((?:[^"\\]|\\.)*)"', re.S)


def literals(src):
    cut = src.find("#[cfg(test)]")
    if cut >= 0:
        src = src[:cut]
    out = []
    for m in LITERAL.finditer(src):
        text = m.group(1)
        if not JAPANESE.search(text):
            continue
        line_start = src.rfind("\n", 0, m.start()) + 1
        if src[line_start : m.start()].lstrip().startswith("//"):
            continue
        text = re.sub(r"\\\n\s*", "", text).replace("\\n", "\n").replace('\\"', '"')
        out.append(text.strip())
    return out


def lint(paragraphs):
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".txt", delete=False) as f:
        f.write("\n\n".join(paragraphs) + "\n")
        path = f.name
    try:
        proc = subprocess.run(
            [sys.executable, "-I", LINT, "--json", "--no-words", "--doc-type", "mail", path],
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        os.unlink(path)
    if proc.returncode not in (0, 1):
        sys.exit(f"check-human-text: yomiyasu_lint.py failed ({proc.returncode}): {proc.stderr}")
    return [f for f in json.loads(proc.stdout)["findings"] if f["severity"] in ("warn", "error")]


def allow_list():
    entries = set()
    with open(ALLOW, encoding="utf-8") as f:
        for raw in f:
            if not raw.strip() or raw.startswith("#"):
                continue
            parts = raw.rstrip("\n").split("\t")
            if len(parts) != 3 or not parts[2].strip():
                sys.exit(f"check-human-text: bad allow line (rule<TAB>snippet<TAB>reason): {raw!r}")
            entries.add((parts[0], parts[1]))
    return entries


def self_test():
    found = {f["rule"] for f in lint(["状況の解像度を上げ、手触りを確かめました。"])}
    if "slop_vocabulary" not in found:
        sys.exit(f"check-human-text --self-test: slop_vocabulary not found ({sorted(found)})")
    print("check-human-text --self-test: ok")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    if sys.argv[1:]:
        sys.exit("usage: check-human-text.py [--self-test]")
    allowed = allow_list()
    used = set()
    bad = []
    total = 0
    for rel in FILES:
        with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
            texts = literals(f.read())
        total += len(texts)
        for finding in lint(texts) if texts else []:
            key = (finding["rule"], finding["snippet"])
            if key in allowed:
                used.add(key)
            else:
                bad.append(f"{rel}: [{finding['rule']}] {finding['snippet']}  ({finding['message']})")
    stale = sorted(allowed - used)
    for rule, snippet in stale:
        bad.append(f"{os.path.relpath(ALLOW, ROOT)}: no longer found: {rule}\t{snippet}")
    if bad:
        print("check-human-text: yomiyasu_lint.py findings in fixed human-facing texts:", file=sys.stderr)
        for line in bad:
            print(f"  {line}", file=sys.stderr)
        sys.exit(1)
    print(f"check-human-text: {len(FILES)} files, {total} literals, 0 findings ({len(used)} allowed)")


if __name__ == "__main__":
    main()
