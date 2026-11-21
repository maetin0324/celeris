#!/bin/sh
# userns の 9 件が env 無しの build で ignored（skipped）と数えられ、pass と数えられないことを確かめる
# （ADR 2026-11-21-userns-tests-explicit-skip の I1）。POSIX sh（dash で動く）。
#
#   sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh   （repo の root で、引数なし）
#
# 1. 4 変数（CELERIS_USERNS_TESTS・CELERIS_ISOLATION_TESTS・CELERIS_DB_GUARD_TESTS・CELERIS_LAUNCHER_TESTS）を
#    unset して cargo nextest list --message-format json を取り、9 件が在って全て ignored か。
# 2. 同じ env で 9 件の filterset を nextest run し、9 件が SKIP の行に出て、1 件も PASS・FAIL の行に出ないか。
# 両方が成り立つときだけ exit 0。cargo は呼び出した env の CARGO_TARGET_DIR を使う。
set -eu

root=$(git rev-parse --show-toplevel)
cd "$root"

work=$(mktemp -d "${TMPDIR:-/tmp}/check-skip.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

no_userns_env() {
    env -u CELERIS_USERNS_TESTS -u CELERIS_ISOLATION_TESTS -u CELERIS_DB_GUARD_TESTS -u CELERIS_LAUNCHER_TESTS "$@"
}

# 9 件（binary id と試験名を tab で区切る）。
cat >"$work/targets.tsv" <<'EOF'
task-worker	db_guard::tests::user_systemd_bus_is_hidden_from_launched_process
task-worker	db_guard::tests::host_config_read_only_inside_namespace
task-worker	db_guard::tests::the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable
task-worker	db_guard::tests::a_nested_user_namespace_cannot_undo_the_read_only_mount
task-worker	db_guard::tests::the_guarded_process_has_no_capabilities_and_keeps_its_uid
task-worker	db_guard::tests::probe_confirms_the_db_is_not_writable_inside
task-worker	db_guard::tests::ssh_config_includes_still_parse_inside_the_namespace
celeris-credentiald::injection_peer_uid	inner_peer_uid_probe
celeris-credentiald::injection_peer_uid	real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch
EOF

filter=$(python3 -I - "$work/targets.tsv" <<'EOF'
import sys
groups = {}
for line in open(sys.argv[1], encoding="utf-8"):
    bid, name = line.rstrip("\n").split("\t")
    groups.setdefault(bid, []).append(name)
print(" | ".join(
    "(binary_id(%s) & (%s))" % (bid, " | ".join("test(=%s)" % n for n in names))
    for bid, names in groups.items()
))
EOF
)

pkgs="-p task-worker --lib -p celeris-credentiald --test injection_peer_uid"

echo "check-skip: nextest list (userns env unset)"
# shellcheck disable=SC2086
if ! no_userns_env cargo nextest list $pkgs --message-format json >"$work/list.json" 2>"$work/list.err"; then
    tail -n 30 "$work/list.err" >&2
    echo "check-skip: FAIL: cargo nextest list failed" >&2
    exit 1
fi

if ! python3 -I - "$work/list.json" "$work/targets.tsv" <<'EOF'
import json, sys
suites = json.load(open(sys.argv[1], encoding="utf-8"))["rust-suites"]
bad = 0
for line in open(sys.argv[2], encoding="utf-8"):
    bid, name = line.rstrip("\n").split("\t")
    case = suites.get(bid, {}).get("testcases", {}).get(name)
    if case is None:
        print("list: MISSING %s %s" % (bid, name))
        bad += 1
    elif case.get("ignored") is not True:
        print("list: NOT IGNORED %s %s %s" % (bid, name, json.dumps(case)))
        bad += 1
    else:
        print("list: ignored %s %s" % (bid, name))
sys.exit(1 if bad else 0)
EOF
then
    echo "check-skip: FAIL: some of the 9 tests are missing or not ignored without the userns env" >&2
    exit 1
fi

echo "check-skip: nextest run (userns env unset)"
# 9 件が全て ignored なら走る試験は 0 件なので --no-tests=warn（在ることは上の list で確かめた）。
# shellcheck disable=SC2086
if ! no_userns_env cargo nextest run $pkgs --no-tests=warn --status-level all --final-status-level all \
    -E "$filter" >"$work/run.out" 2>&1; then
    tail -n 40 "$work/run.out" >&2
    echo "check-skip: FAIL: cargo nextest run failed" >&2
    exit 1
fi

if ! python3 -I - "$work/run.out" "$work/targets.tsv" <<'EOF'
import re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
status = {}
for line in text.splitlines():
    m = re.match(r"\s*(PASS|FAIL|SKIP|SIGSEGV|SIGABRT|TIMEOUT|LEAK|FLAKY|TRY \d+ \w+)\s*\[[^\]]*\]\s*(?:\([^)]*\)\s*)?(\S+)\s+(\S+)\s*$", line)
    if m:
        status.setdefault((m.group(2), m.group(3)), set()).add(m.group(1))
bad = 0
for line in open(sys.argv[2], encoding="utf-8"):
    bid, name = line.rstrip("\n").split("\t")
    seen = status.get((bid, name), set())
    if seen != {"SKIP"}:
        print("run: %s %s -> %s (want only SKIP)" % (bid, name, sorted(seen) or "no status line"))
        bad += 1
    else:
        print("run: SKIP %s %s" % (bid, name))
summary = [l.strip() for l in text.splitlines() if l.strip().startswith("Summary")]
print("run: " + (summary[-1] if summary else "no Summary line"))
sys.exit(1 if bad else 0)
EOF
then
    tail -n 40 "$work/run.out" >&2
    echo "check-skip: FAIL: some of the 9 tests were not counted as skipped" >&2
    exit 1
fi

echo "check-skip: ok (9 tests ignored in the list and skipped in the run; none passed)"
