#!/bin/sh
# routing_routellm_runbook_pins_dependencies_and_license
# Checks docs/ops/model-routing-migration.md §10 (RouteLLM sidecar) against
# requirements.lock and the repository. Offline and read-only.
#   (no args)            pins, headings, references, commands, approval format
#   --require-approved   additionally fails while routellm-weights-use is pending
set -u

NAME=routing_routellm_runbook_pins_dependencies_and_license
require_approved=0
case "${1:-}" in
  '') ;;
  --require-approved) require_approved=1 ;;
  *) echo "usage: check-runbook.sh [--require-approved]" >&2; exit 2 ;;
esac

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
doc="$root/docs/ops/model-routing-migration.md"
lock="$root/scripts/model-routing/requirements.lock"
sidecar="$root/scripts/model-routing/routellm_sidecar.py"
fails=0
fail() { echo "$NAME: FAIL: $*"; fails=$((fails + 1)); }

for f in "$doc" "$lock" "$sidecar" "$root/scripts/model-routing/real-sidecar-check.sh"; do
  [ -f "$f" ] || { echo "$NAME: FAIL: missing $f"; exit 1; }
done

# The RouteLLM section: from its heading to the next level-2 heading.
section=$(awk '/^## 10\. Phase 5: RouteLLM sidecar/{on=1; print; next} on && /^## /{exit} on{print}' "$doc")
[ -n "$section" ] || { echo "$NAME: FAIL: heading '## 10. Phase 5: RouteLLM sidecar' not found"; exit 1; }

for h in '### 10.1 版と出所の記録' '### 10.2 license と notice' '### 10.3 承認記録' \
         '### 10.4 依存の pin と構築' '### 10.5 CPU / GPU 要件' '### 10.6 起動・確認・停止' \
         '### 10.7 Celeris への接続'; do
  printf '%s\n' "$section" | grep -q "^$h" || fail "missing heading: $h"
done

field() { printf '%s\n' "$section" | sed -n "s/^$1: *//p" | head -n 1; }

# Pins: every package pin in the lock appears in the section, and every pin in
# the section matches the lock.
lock_sha=$(sed -n 's/^routellm @ git+https:\/\/github\.com\/lm-sys\/RouteLLM\.git@\([0-9a-f]\{40\}\)$/\1/p' "$lock")
[ -n "$lock_sha" ] || fail "requirements.lock has no full RouteLLM commit"
[ "$(field routellm-commit)" = "$lock_sha" ] || fail "routellm-commit '$(field routellm-commit)' != lock '$lock_sha'"
py_pin=$(sed -n 's/^# \(python==[0-9.]*\).*/\1/p' "$lock")
[ -n "$py_pin" ] || fail "requirements.lock has no '# python==' pin"
for pkg in torch transformers litellm; do
  pin=$(grep -E "^$pkg==" "$lock")
  [ -n "$pin" ] || { fail "requirements.lock has no $pkg pin"; continue; }
  printf '%s\n' "$section" | grep -qF "\`$pin\`" || fail "section lacks lock pin $pin"
done
[ -z "$py_pin" ] || printf '%s\n' "$section" | grep -qF "\`$py_pin\`" || fail "section lacks lock pin $py_pin"
for pin in $(printf '%s\n' "$section" | grep -oE '(python|torch|transformers|litellm)==[0-9][0-9.a-z]*' | sort -u); do
  case $pin in
    python==*) [ "$pin" = "$py_pin" ] || fail "section pin $pin differs from lock" ;;
    *) grep -qxF "$pin" "$lock" || fail "section pin $pin differs from lock" ;;
  esac
done

# Provenance and license fields.
wrapper=$(field wrapper-revision)
if ! printf '%s' "$wrapper" | grep -qE '^[0-9a-f]{40}$'; then
  fail "wrapper-revision is not a full SHA: '$wrapper'"
elif command -v git >/dev/null 2>&1 && git -C "$root" rev-parse --git-dir >/dev/null 2>&1; then
  git -C "$root" cat-file -e "$wrapper^{commit}" 2>/dev/null || fail "wrapper-revision $wrapper not found in repository"
fi
for k in weights-repository weights-revision weights-sha256 tokenizer-repository tokenizer-revision tokenizer-sha256; do
  [ -n "$(field "$k")" ] || fail "missing field $k"
done
for word in Apache-2.0 'weights / tokenizer' torch transformers litellm notice; do
  printf '%s\n' "$section" | grep -qF -- "$word" || fail "license/notice table lacks '$word'"
done
printf '%s\n' "$section" | grep -qF '未検証の CPU/GPU 構成を保証しない' || fail "CPU/GPU section lacks the no-guarantee statement"
for word in OMP_NUM_THREADS VRAM CUDA; do
  printf '%s\n' "$section" | grep -qF "$word" || fail "CPU/GPU section lacks '$word'"
done

# Approval state.
state=$(field routellm-weights-use)
case "$state" in
  approved)
    for k in weights-revision weights-sha256 tokenizer-revision tokenizer-sha256 approval-date approver terms-evidence; do
      v=$(field "$k")
      case "$v" in ''|未記入*) fail "approved but $k is not recorded" ;; esac
    done
    printf '%s' "$(field weights-revision)" | grep -qE '^[0-9a-f]{40}$' || fail "approved but weights-revision is not a full SHA"
    printf '%s' "$(field weights-sha256)" | grep -qE '^[0-9a-f]{64}$' || fail "approved but weights-sha256 is not a SHA-256"
    ;;
  pending)
    [ "$require_approved" -eq 0 ] || fail "routellm-weights-use is pending (--require-approved)"
    ;;
  *) fail "routellm-weights-use must be approved or pending, got '$state'" ;;
esac

# Commands and expected results named in the procedure.
for word in 'routellm_sidecar.py' '--host 127.0.0.1' '--router bert' '--weights-dir' '--strong' '--weak' \
            'READY port=' '/healthz' '/estimate' 'kill -TERM' 'kill -0' 'ss -ltn' 'real-sidecar-check.sh start-stop' \
            'enabled = false' 'shadow_only = true' 'send_prompt' 'prompt_allowlist' 'daily_max_requests' \
            '[model_routing.estimator.sidecar.allowlist]' 'timeout_ms' 'POST /api/v1/reload' 'pip install -r scripts/model-routing/requirements.lock'; do
  printf '%s\n' "$section" | grep -qF -- "$word" || fail "procedure lacks '$word'"
done
# The flags and markers the procedure uses must exist in the wrapper.
for word in '"--host"' '"--port"' '"--router"' '"--weights-dir"' '"--strong"' '"--weak"' '"--fake-classifier"' \
            'READY port=' '"/healthz"' '"/estimate"' 'SIGTERM' 'HF_HUB_OFFLINE'; do
  grep -qF -- "$word" "$sidecar" || fail "routellm_sidecar.py lacks $word used by the runbook"
done
# Config keys the procedure writes must exist in the config schema.
cfg="$root/crates/celeris/src/config/model_routing.rs"
if [ -f "$cfg" ]; then
  for key in enabled shadow_only endpoint estimator_id estimator_version timeout_ms max_inflight send_prompt daily_max_requests; do
    grep -qE "pub $key:" "$cfg" || fail "config key $key not in model_routing.rs"
  done
else
  fail "missing $cfg"
fi

# References: repository paths and relative links in the section must resolve.
for p in $(printf '%s\n' "$section" | grep -oE '(scripts|docs|crates|agent-docs)/[A-Za-z0-9._/-]+\.(sh|py|lock|md|rs)' | sort -u); do
  case $p in
    docs/reports/model-routing-routellm-shadow.md) ;;  # created by the real evaluation
    *) [ -e "$root/$p" ] || fail "referenced path not found: $p" ;;
  esac
done
for l in $(printf '%s\n' "$section" | grep -oE '\]\(\.\.?/[^)#]+' | sed 's/^](//' | sort -u); do
  [ -e "$root/docs/ops/$l" ] || fail "relative link not found: $l"
done

if [ "$fails" -ne 0 ]; then
  echo "$NAME: FAILED ($fails)"
  exit 1
fi
echo "$NAME: ok (routellm $lock_sha, routellm-weights-use=$state)"
exit 0
