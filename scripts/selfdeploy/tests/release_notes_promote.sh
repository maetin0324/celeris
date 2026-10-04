#!/bin/sh
# scripts/selfdeploy/tests/release_notes_promote.sh — ADR 2026-10-04-release-notes
#
#   `celerisctl release notes` と `celerisctl release preview` を、一時 git repo と一時 releases ディレクトリで確かめる。
#   base → (task branch を --no-ff で merge) → 直接の commit → migration の履歴に対し、
#     - リリース A（merge の commit まで）と B（先端まで）の notes を作り、
#     - `current` を base のリリースに向けて、B の preview が task を 1 回だけ数え、A と B を両方含むこと。
#   promote.sh 自体は systemctl 等の偽物が大量に要る（promote_authorization_marker.sh 参照）ので、ここでは回さない。
#
# 本番には触れない: HOME / CELERIS_CONFIG_DIR / CELERIS_STATE_DIR は一時ディレクトリ。ネットワークは使わない（--api なし）。
#
# 実行: sh scripts/selfdeploy/tests/release_notes_promote.sh   （CELERISCTL=<bin> で celerisctl を指定できる）
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }
assert_eq() {
  if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (want=[$2] got=[$3])"; fi
}

# ビルドは本物の HOME で（rustup が要る）。以降は HOME を一時ディレクトリに替える。
CTL="${CELERISCTL:-}"
if [ -z "$CTL" ]; then
  if [ -z "${CARGO_TARGET_DIR:-}" ]; then echo "set CELERISCTL or CARGO_TARGET_DIR" >&2; exit 2; fi
  (cd "$ROOT" && cargo build -q -p celerisctl) || { echo "cargo build failed" >&2; exit 2; }
  CTL="$CARGO_TARGET_DIR/debug/celerisctl"
fi
[ -x "$CTL" ] || { echo "no celerisctl at $CTL" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
export CELERIS_CONFIG_DIR="$WORK/config"
export CELERIS_STATE_DIR="$WORK/state"
mkdir -p "$HOME" "$CELERIS_CONFIG_DIR" "$CELERIS_STATE_DIR"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com

json() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"; }

TASK=01ARZ3NDEKTSV4RRFFQ69G5FAV
REPO="$WORK/repo"
mkdir -p "$REPO"
cd "$REPO"
git init -q -b main
echo base >a.txt; git add -A; git commit -q -m base
BASE="$(git rev-parse HEAD)"
git checkout -q -b "celeris/$TASK"
echo 1 >t1.txt; git add -A; git commit -q -m "task one"
echo 2 >t2.txt; git add -A; git commit -q -m "task two"
git checkout -q main
git merge --no-ff -q -m "Merge branch 'celeris/$TASK'" "celeris/$TASK"
MID="$(git rev-parse HEAD)"
echo d >direct.txt; git add -A; git commit -q -m "direct change"
mkdir -p crates/task-core/migrations
echo '-- m' >crates/task-core/migrations/0099_x.sql; git add -A; git commit -q -m "add migration"
HEAD_SHA="$(git rev-parse HEAD)"
cd "$WORK"

REL_ROOT="$CELERIS_STATE_DIR/releases"
mkdir -p "$REL_ROOT"
mk_release() { # sha schema [notes-base]
  sha="$1"; schema="$2"; s12="$(printf '%s' "$sha" | cut -c1-12)"
  d="$REL_ROOT/$s12"; mkdir -p "$d"
  printf '{"sha":"%s","sha12":"%s","built_at":"%s","schema_version":%s}\n' "$sha" "$s12" "$3" "$schema" >"$d/manifest.json"
  if [ -n "${4:-}" ]; then
    printf '{"steps":[{"step":"pnpm-test","skipped":true,"reason":"gui unchanged"}]}\n' >"$d/gate.json"
    "$CTL" release notes --repo "$REPO" --sha "$sha" --base "$4" --schema-from 1 --schema-to "$schema" \
      --gate-json "$d/gate.json" --out-dir "$d" >"$WORK/notes.out"
  fi
  echo "$s12"
}
BASE12="$(mk_release "$BASE" 1 2026-10-01T00:00:00Z)"
A12="$(mk_release "$MID" 1 2026-10-02T00:00:00Z "$BASE")"
B12="$(mk_release "$HEAD_SHA" 2 2026-10-03T00:00:00Z "$BASE")"
ln -s "releases/$BASE12" "$CELERIS_STATE_DIR/current"

[ -f "$REL_ROOT/$B12/notes.json" ] && [ -f "$REL_ROOT/$B12/notes.md" ] && ok "notes.json and notes.md written" || ng "notes files missing"
[ ! -e "$REL_ROOT/$B12/.notes.json.tmp" ] && ok "no tmp file left" || ng "tmp file left"
assert_eq "B notes: one task" "[\"$TASK\"]" "$(json "$REL_ROOT/$B12/notes.json" '[t["task_id"] for t in d["tasks"]]' | sed "s/'/\"/g")"
assert_eq "B notes: one migration" 1 "$(json "$REL_ROOT/$B12/notes.json" 'len(d["migrations"])')"
assert_eq "B notes: schema changed" True "$(json "$REL_ROOT/$B12/notes.json" 'd["schema"]["changed"]')"
assert_eq "B notes: gate skip recorded" 1 "$(json "$REL_ROOT/$B12/notes.json" 'len(d["gate_skips"])')"
grep -q "$TASK" "$REL_ROOT/$B12/notes.md" && ok "notes.md names the task" || ng "notes.md lacks task id"
case "$(cat "$WORK/notes.out")" in *tasks=1*) ok "summary line has tasks=1";; *) ng "summary line: $(cat "$WORK/notes.out")";; esac

"$CTL" release preview "$B12" --releases-dir "$REL_ROOT" --json >"$WORK/preview.json"
assert_eq "preview: task exactly once" 1 "$(json "$WORK/preview.json" 'len([t for t in d["tasks"] if t["task_id"]=="'"$TASK"'"])')"
assert_eq "preview: includes A and B" "True" "$(json "$WORK/preview.json" 'set(r["sha12"] for r in d["releases"]) >= {"'"$A12"'","'"$B12"'"}')"
assert_eq "preview: complete" True "$(json "$WORK/preview.json" 'd["complete"]')"
assert_eq "preview: from current" "$BASE12" "$(json "$WORK/preview.json" 'd["from"]')"

"$CTL" release preview "$B12" --releases-dir "$REL_ROOT" >"$WORK/preview.md"
grep -q "$TASK" "$WORK/preview.md" && ok "markdown preview lists the task" || ng "markdown preview lacks task"

if "$CTL" release preview ffffffffffff --releases-dir "$REL_ROOT" >/dev/null 2>"$WORK/err"; then
  ng "unknown release should exit non-zero"
else
  ok "unknown release exits non-zero"
fi
if "$CTL" release notes --repo "$REPO" --sha deadbeefdeadbeefdeadbeefdeadbeefdeadbeef --out-dir "$WORK/o" >/dev/null 2>&1; then
  ng "unknown sha should fail"
else
  ok "unknown sha exits non-zero"
fi

[ "$FAIL" -eq 0 ] && echo "all ok" || { echo "FAILED" >&2; exit 1; }
