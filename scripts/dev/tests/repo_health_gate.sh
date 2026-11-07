#!/bin/sh
# check-health.sh（repo health の単一入口）のゲートの試験（ADR 2026-11-07-repo-health-celeris-profile D4）。
# 一時 git repo に check-health.sh・repo-health と小さい profile・包む検査 1 本を置き、次を確かめる:
#   (1) 必須 rule が pass なら exit 0、report は validate_report.py を通る
#   (2) 必須 rule の fail（包んだ検査の非 0）で exit 1
#   (3) 欠測: shallow clone で base に届かなければ exit 2（全 rule unsupported）
#   (4) policy の弱化（必須 rule を外す・包んだ検査を exit 0 に書き換える）で exit 1（policy.changed）
#   (5) doc-only の変更は軽量経路: 構造の rule（source-size）の command を起こさず、unit 試験も走らせず exit 0
# 外部ネットワークに出ない。CPU を焼く負荷はかけない。
set -eu

repo_root=$(cd "$(dirname "$0")/../../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/repo-health-gate.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset CELERIS_WU_BASE 2>/dev/null || true

fail() { echo "repo_health_gate: FAIL $*" >&2; exit 1; }

repo="$tmp/repo"
mkdir -p "$repo/scripts/dev" "$repo/scripts/tests" "$repo/docs"
cp "$repo_root/scripts/dev/check-health.sh" "$repo/scripts/dev/"
cp -R "$repo_root/scripts/dev/repo-health" "$repo/scripts/dev/"
# deps.cycle・size.* は source-size-report.py の分類を使う（無ければ unsupported で exit 2）。
cp "$repo_root/scripts/dev/source-size-report.py" "$repo_root/scripts/dev/source-size-report.toml" "$repo/scripts/dev/"
# 包む検査: BAD という file があれば落ちる。構造の検査は呼ばれたら印を残す。
cat >"$repo/scripts/dev/check-bad.sh" <<'EOF'
#!/bin/sh
[ ! -e "$1/BAD" ] || { echo "BAD:1: BAD がある"; exit 1; }
EOF
cat >"$repo/scripts/dev/check-structure.sh" <<'EOF'
#!/bin/sh
: >"$GATE_MARK"
EOF
cat >"$repo/scripts/dev/repo-health/profiles/default.toml" <<'EOF'
version = "1"
policy_paths = ['^scripts/dev/repo-health/', '^scripts/dev/check-(health|bad|structure)\.sh$']
exclude_paths = ['^scripts/']
[rules."doc.links"]
required = true
commands = [["sh", "scripts/dev/check-bad.sh", "{root}"]]
[rules."source-size"]
required = true
commands = [["sh", "scripts/dev/check-structure.sh"]]
EOF
echo '# readme' >"$repo/README.md"
echo 'print("x")' >"$repo/app.py"
cd "$repo"
git init -q -b main
git add -A
git commit -q -m base
git switch -q -c task

GATE_MARK="$tmp/structure-ran"
export GATE_MARK
out="$tmp/report.json"

run() {  # run <want exit> <label> [check-health の引数]
    want=$1; label=$2; shift 2
    rm -f "$out" "$GATE_MARK"
    rc=0
    bash scripts/dev/check-health.sh --out "$out" "$@" >"$tmp/log" 2>&1 || rc=$?
    [ "$rc" -eq "$want" ] || { cat "$tmp/log" >&2; fail "$label: exit $rc, want $want"; }
    if [ -f "$out" ]; then
        python3 -I "$repo_root/scripts/dev/repo-health/validate_report.py" "$out" >/dev/null \
            || fail "$label: report が契約に合わない"
    fi
}
field() { python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(eval(sys.argv[2], {"r": r}))' "$out" "$1"; }

# (1) 合格: コードの変更（app.py）で full、必須 rule は pass
echo 'print("y")' >app.py
run 0 "(1) pass"
[ "$(field 'r["inputs"]["mode"]')" = full ] || fail "(1) mode が full でない"
[ -f "$GATE_MARK" ] || fail "(1) full で構造の検査が走っていない"
git commit -qam "code"
echo "OK (1) required rules pass -> exit 0"

# (2) 必須 rule の fail
: >BAD
run 1 "(2) fail"
[ "$(field '[x["status"] for x in r["rules"] if x["id"] == "doc.links"][0]')" = fail ] || fail "(2) doc.links が fail でない"
grep -q 'BAD:1' "$tmp/log" || fail "(2) 証拠の位置が出ていない"
rm BAD
echo "OK (2) failing required rule -> exit 1"

# (3) 欠測: shallow clone で base の commit に届かない
base_sha=$(git rev-parse main)
git clone -q --depth 1 --branch task "file://$repo" "$tmp/shallow"
( cd "$tmp/shallow" && rc=0 && bash scripts/dev/check-health.sh --base "$base_sha" --out "$out" >"$tmp/log" 2>&1 || rc=$?
  [ "$rc" -eq 2 ] || { cat "$tmp/log" >&2; exit 1; } ) || fail "(3) shallow: exit が 2 でない"
[ "$(field 'r["snapshot"]["shallow"]')" = True ] || fail "(3) shallow が記録されていない"
[ "$(field 'r["summary"]["verdict"]')" = error ] || fail "(3) verdict が error でない"
echo "OK (3) shallow clone (base unreachable) -> exit 2"

# (4) policy の弱化: 必須 rule を外す・包んだ検査を exit 0 にする。どちらも base の policy で評価して落ちる
sed -i 's/^required = true$/required = false/' scripts/dev/repo-health/profiles/default.toml
: >BAD
run 1 "(4a) profile weakening"
[ "$(field 'r["policy"]["changed"]')" = True ] || fail "(4a) policy.changed が立っていない"
[ "$(field '[x["status"] for x in r["rules"] if x["id"] == "doc.links"][0]')" = fail ] \
    || fail "(4a) base の policy（required）で doc.links が評価されていない"
git checkout -q -- scripts/dev/repo-health/profiles/default.toml
printf '#!/bin/sh\nexit 0\n' >scripts/dev/check-bad.sh
run 1 "(4b) checker weakening"
[ "$(field '[x["status"] for x in r["rules"] if x["id"] == "policy.changed"][0]')" = fail ] \
    || fail "(4b) policy.changed が fail でない"
git checkout -q -- scripts/dev/check-bad.sh
rm BAD
echo "OK (4) weakening the profile or a wrapped checker -> exit 1 (policy.changed)"

# (5) doc-only: 文書だけの変更は構造の検査と unit 試験を起こさない（main を進め、base..作業ツリーを文書だけにする）
git branch -f main HEAD
echo 'more' >>README.md
mkdir -p docs && echo '# guide' >docs/guide.md
run 0 "(5) doc-only"
[ "$(field 'r["inputs"]["mode"]')" = doc-only ] || fail "(5) mode が doc-only でない"
[ "$(field '[x["status"] for x in r["rules"] if x["id"] == "source-size"][0]')" = not_applicable ] \
    || fail "(5) source-size が not_applicable でない"
[ ! -f "$GATE_MARK" ] || fail "(5) doc-only で構造の検査が走った"
grep -q 'doc-only（検査器の unit 試験を省く）' "$tmp/log" || fail "(5) 軽量経路で終わっていない"
echo "OK (5) doc-only change takes the light path"

echo "repo_health_gate: all checks passed"
