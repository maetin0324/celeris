#!/usr/bin/env bash
# generic-loop.sh — Celeris でない fixture repo で repo health の loop を 1 周させる（健全性v2 V0 generic）。
#
#   bash agent-docs/progress/2026-11-15-health-v2-v0/generic-loop.sh
#
# 引数なし。作業は $TMPDIR の下の一時 dir だけで行い、終わると消す。外部ネットワーク・LLM・実ユーザーの repo は
# 使わない。HEAD の検査器（scripts/dev/repo-health/repo_health.py）と G4 の fixture
# （scripts/tests/repo_health_fixtures.py の ts/dep-inversion）をそのまま呼び、どちらも写さない・変えない。
#
# 1 つの fixture repo（main = base、candidate = 候補）で次の順に変え、各段で検査器を
# `--base main --head candidate` で当てる（各段とも status=fail の rule を数え、not_applicable は数えない。
# error・unknown・unsupported の rule と policy.changed=true は、どの段でも 0 件のはず）。
#   1 boundary  core が app を import する（fixture の dep-inversion のまま）
#               → exit 1、fail は deps.violation だけ（例外なし）
#   2 expired   main がその finding の fingerprint に期限付きの例外（expires 2026-02-01）を 2026-01-15 に置き、
#               main は 2026-03-02 に進む。候補を main に rebase する
#               → exit 1、fail は deps.violation だけ。finding は exception=null で expired_exception（base の
#                 profile の行）を持ち、基準日 policy.exceptions_as_of は base の commit の日付 2026-03-02
#   3 duplicate 逆依存を外し、core の Session と同じ形（id・owner・state）の SessionView を app に足す
#               → exit 0。dup.contract の new の候補（warning、fail 級でない、evidence は 2 file）が report に出る。
#                 dup.* は検査器の契約で review の候補で、不合格にするのは候補を根拠にした reviewer の blocking
#                 （task-dispatch の g4_r1_review_request_ts_dup_contract_fails_review が確かめる）
#   4 fixed     SessionView をやめ、core の Session を import して使う
#               → exit 0、verdict pass、deps.violation・dup.contract の finding は 0 件
# 各段の report は validate_report.py で契約（schema と意味の検査）に合うことも確かめる。
# 段の結果が期待と違えば（順が崩れたら）、その段の要点を出して exit 1。全段が期待どおりなら exit 0。
set -euo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
CHECKER=$ROOT/scripts/dev/repo-health/repo_health.py
VALIDATOR=$ROOT/scripts/dev/repo-health/validate_report.py
FIXTURES=$ROOT/scripts/tests/repo_health_fixtures.py
PROFILE=scripts/dev/repo-health/profiles/default.toml
: "${TMPDIR:?TMPDIR が要る（作業は TMPDIR の下で行う）}"

WORK=$(mktemp -d "$TMPDIR/generic-loop.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
REPO=$WORK/orchard
mkdir -p "$WORK/home" "$WORK/reports"

# 呼び出し側の GIT_*・global/system の git 設定・hook を引き継がない（fixture の git() と同じ隔離）。
clean() {
    env -i PATH="$PATH" HOME="$WORK/home" TMPDIR="$TMPDIR" LANG=C.UTF-8 \
        GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$@"
}
# g <date> <git args...>: fixture repo の git を、作者・committer の日付を固定して呼ぶ。
g() {
    local date=$1
    shift
    clean GIT_AUTHOR_DATE="$date" GIT_COMMITTER_DATE="$date" git -C "$REPO" -c core.hooksPath=/dev/null "$@"
}
commit() {
    g "$1" add -A
    g "$1" commit -q -m "$2"
}

# step <n> <label> <expect_exit> <expect_fail_rules（カンマ区切り、無ければ -）> <mode: boundary|expired|duplicate|fixed>
step() {
    local n=$1 label=$2 want_exit=$3 want_fail=$4 mode=$5
    local out=$WORK/reports/$n-$label.json rc=0
    clean python3 -I "$CHECKER" --repo "$REPO" --base main --head candidate --policy-version 1 \
        --out "$out" >"$WORK/reports/$n-$label.stdout" 2>"$WORK/reports/$n-$label.stderr" || rc=$?
    echo "== step $n $label: checker exit=$rc"
    # not_applicable の rule（arch-map.*・wrapped checks）の行は省く。全 rule の status は下の rules: の行にある。
    sed -E 's/ base=[0-9a-f]+ head=[0-9a-f]+//; / not_applicable /d; s/^/   | /' "$WORK/reports/$n-$label.stdout"
    if [ ! -f "$out" ]; then
        echo "step $n $label: report が無い（exit=$rc）" >&2
        sed 's/^/   ! /' "$WORK/reports/$n-$label.stderr" >&2
        exit 1
    fi
    clean python3 -I "$VALIDATOR" "$out" >"$WORK/reports/$n-$label.validate" 2>&1 || {
        echo "step $n $label: report が契約に合わない" >&2
        sed 's/^/   ! /' "$WORK/reports/$n-$label.validate" >&2
        exit 1
    }
    clean python3 -I - "$out" "$rc" "$n" "$label" "$want_exit" "$want_fail" "$mode" "$PROFILE" <<'PY'
import json
import sys

path, rc, n, label, want_exit, want_fail, mode, profile = sys.argv[1:]
rep = json.load(open(path, encoding="utf-8"))
fail_rules = sorted(r["id"] for r in rep["rules"] if r["status"] == "fail")
# 必須 rule の error・unknown・unsupported は exit 2 の側。この loop では 1 件も出ないはず
broken = sorted(r["id"] for r in rep["rules"] if r["status"] in ("error", "unknown", "unsupported"))
# engine.is_fail_grade と同じ式: severity=error・delta が new か worsened・exception が null
grade = [f for f in rep["findings"]
         if f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None]
by_rule = {}
for f in rep["findings"]:
    by_rule.setdefault(f["rule_id"], []).append(f)
s = rep["summary"]
expired = [f for f in rep["findings"] if f.get("expired_exception")]
as_of = rep["policy"].get("exceptions_as_of", "-")
dups = by_rule.get("dup.contract", [])
print(f"   step={n} label={label} exit={rc} verdict={s['verdict']} summary.exit_code={s['exit_code']}"
      f" reviewable={str(s['reviewable']).lower()}"
      f" fail_rules={','.join(fail_rules) or '-'}"
      f" fail_grade={','.join(sorted(f['rule_id'] for f in grade)) or '-'}"
      f" expired_exception={len(expired)} exceptions_as_of={as_of}"
      f" dup.contract={len(dups)}{'(' + ','.join(f['severity'] + '/' + f['delta'] for f in dups) + ')' if dups else ''}"
      f" policy.changed={str(rep['policy']['changed']).lower()}")
print("   rules: " + " ".join(f"{r['id']}={r['status']}" for r in rep["rules"] if r["status"] != "not_applicable"))
problems = []
if int(rc) != int(want_exit) or s["exit_code"] != int(want_exit):
    problems.append(f"exit は {want_exit} のはず（checker {rc}、report {s['exit_code']}）")
want = [] if want_fail == "-" else want_fail.split(",")
if fail_rules != sorted(want):
    problems.append(f"fail の rule は {want} のはず（{fail_rules}）")
if sorted({f["rule_id"] for f in grade}) != sorted(want):
    problems.append(f"fail 級の finding の rule は {want} のはず")
if broken:
    problems.append(f"error・unknown・unsupported の rule は無いはず（{broken}）")
if rep["policy"]["changed"]:
    problems.append("候補は policy の file を変えていないのに policy.changed")
if mode == "expired":
    hits = [f for f in grade if f["rule_id"] == "deps.violation" and f.get("expired_exception")]
    ee = hits[0]["expired_exception"] if hits else {}
    if not hits or ee.get("expires") != "2026-02-01" or ee.get("source", {}).get("path") != profile:
        problems.append(f"deps.violation の finding に base の期限切れの例外（expires 2026-02-01）が付くはず（{ee}）")
    if as_of != "2026-03-02":
        problems.append(f"基準日は base の commit の日付 2026-03-02 のはず（{as_of}）")
elif expired:
    problems.append("期限切れの例外はこの段に出ないはず")
if mode == "duplicate":
    paths = sorted({e["path"] for f in dups for e in f["evidence"]})
    if not (len(dups) == 1 and dups[0]["candidate"] is True and dups[0]["severity"] == "warning"
            and dups[0]["delta"] == "new" and dups[0]["exception"] is None
            and paths == ["src/app/session_view.ts", "src/core/session.ts"]):
        problems.append(f"dup.contract の new の候補（warning、evidence は 2 file）が 1 件のはず（{dups}、{paths}）")
    if not s["reviewable"]:
        problems.append("候補は reviewer に渡る（reviewable=true）はず")
elif dups:
    problems.append("dup.contract の finding はこの段に出ないはず")
if mode == "fixed" and (by_rule.get("deps.violation") or s["verdict"] != "pass"):
    problems.append("直した候補は deps.violation の finding が無く verdict=pass のはず")
if problems:
    for p in problems:
        print(f"step {n} {label}: {p}", file=sys.stderr)
    sys.exit(1)
PY
}

# 0. fixture: main = base（core・auth・app の 3 層と default profile）、candidate = core→app の逆依存
clean python3 -I "$FIXTURES" build ts dep-inversion "$REPO" >"$WORK/fixture.json"
echo "fixture: ts/dep-inversion（main と candidate の 2 branch、作業は \$TMPDIR の下）"

# 1. 境界違反の変更（fixture の candidate そのまま）
step 1 boundary 1 deps.violation boundary

# 2. 期限切れの例外: main が step 1 の finding の fingerprint に期限付きの例外を置き（2026-01-15、期限内）、
#    main は 2026-03-02 に進む。候補を main に rebase する。policy は base 側で読み、基準日は base の commit の UTC の日付。
fp=$(clean python3 -I -c 'import json, sys
(f,) = [f for f in json.load(open(sys.argv[1]))["findings"] if f["rule_id"] == "deps.violation"]
print(f["fingerprint"])' "$WORK/reports/1-boundary.json")
g 2026-01-15T00:00:00Z switch -q main
cat >>"$REPO/$PROFILE" <<TOML

[[rules."deps.violation".exceptions]]
fingerprint = "$fp"
reason = "移行中: core から app への依存は 2026-02-01 までに外す（期限付き）"
expires = 2026-02-01
TOML
commit 2026-01-15T00:00:00Z 'main: core→app の依存に期限付きの例外を置く'
printf '# Guide\n\nRun the app with the default settings.\n' >"$REPO/docs/guide.md"
commit 2026-03-02T00:00:00Z 'main: guide を足す'
g 2026-03-02T00:00:00Z switch -q candidate
g 2026-03-02T00:00:00Z rebase -q main
step 2 expired 1 deps.violation expired

# 3. 重複の変更: 逆依存を外し、core の Session と同じ形（id・owner・state）の SessionView を app に足す
g 2026-03-03T00:00:00Z rm -q src/core/inverted.ts
cat >"$REPO/src/app/session_view.ts" <<'TS'
export interface SessionView {
  id: string;
  owner: string;
  state: string;
}
TS
commit 2026-03-03T00:00:00Z 'candidate: 逆依存を外し、app に SessionView を足す'
step 3 duplicate 0 - duplicate

# 4. 直した変更: SessionView をやめ、core の Session を使う
cat >"$REPO/src/app/session_view.ts" <<'TS'
import type { Session } from '../core/session';

export function sessionTitle(session: Session): string {
  return `${session.owner}/${session.id}`;
}
TS
commit 2026-03-04T00:00:00Z 'candidate: core の Session を使う'
step 4 fixed 0 - fixed

echo "generic-loop: ok（boundary fail → expired fail → duplicate 候補 → fixed pass）"
