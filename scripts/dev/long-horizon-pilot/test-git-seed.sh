#!/bin/sh
# driver の git seed と stub の commit の試験（LLM・外部 network なし）。使い方: sh scripts/dev/long-horizon-pilot/test-git-seed.sh
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/lh-git-seed.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/seed"
echo seed > "$tmp/seed/README.md"
python3 -I -c "
import importlib.util, sys
spec = importlib.util.spec_from_file_location('driver', '$here/driver.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
m.make_git_seed('$tmp/seed', '$tmp/repo')"
[ "$(git -C "$tmp/repo" rev-parse --abbrev-ref HEAD)" = main ] || { echo "FAIL: branch is not main"; exit 1; }
git -C "$tmp/repo" ls-files --error-unmatch README.md >/dev/null
echo '{"o1":"answer"}' > "$tmp/answers.json"
mkdir "$tmp/art"
printf '{"task":{"kind":"coding","id":"t","acceptance":[{"text":"c-o1: x"}]},"workspace":"%s","artifacts_dir":"%s"}' \
    "$tmp/repo" "$tmp/art" | python3 -I "$here/stub_harness.py" --answers "$tmp/answers.json" >/dev/null
git -C "$tmp/repo" ls-tree -r --name-only main | grep -qx out/o1.txt || { echo "FAIL: out/o1.txt is not on main"; exit 1; }
[ -z "$(git -C "$tmp/repo" status --porcelain)" ] || { echo "FAIL: dirty tree"; exit 1; }
echo "test-git-seed: OK"
