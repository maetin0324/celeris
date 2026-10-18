#!/bin/sh
# 実資産（asset store）で K2/K4 を無 LLM のまま採点できるか確かめる（本測定の前に人が実行する）。
# 使い方: sh crates/campaign/scripts/verify-real-assets.sh <asset_dir> [--build]
#   <asset_dir>   承認済み asset store の k2 dir（SHA256SUMS・benchfs-*.bundle・submodules/*.tar）。
#                 known-good 用に k2-fix.patch（P0 の最終 patch）を置くか CAMPAIGN_K2_FIX_PATCH で指す。
#   CAMPAIGN_K4_TGZ  K4 の原票 tgz（既定: asset_dir/../k4/main-v3-n2-43706.tgz があればそれ）。
#   --build       実 cargo で build/test まで通す（UCX 等の build 環境が要る。不良は env_failure と分かる）。
# 欠落・hash 不一致・build 環境不良は exit 非 0。LLM・有料 job は使わない。
set -eu
dir=${1:?usage: verify-real-assets.sh <asset_dir> [--build]}
[ -d "$dir" ] || { echo "asset dir not found: $dir" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
dir=$(cd "$dir" && pwd)
if [ -f "$dir/SHA256SUMS" ]; then
  (cd "$dir" && sha256sum -c SHA256SUMS) || { echo "asset hash mismatch" >&2; exit 3; }
else
  echo "SHA256SUMS missing in $dir" >&2; exit 3
fi
export CAMPAIGN_ASSET_DIR="$dir"
if [ -z "${CAMPAIGN_K4_TGZ:-}" ] && [ -f "$dir/../k4/main-v3-n2-43706.tgz" ]; then
  CAMPAIGN_K4_TGZ="$dir/../k4/main-v3-n2-43706.tgz"
fi
[ -n "${CAMPAIGN_K4_TGZ:-}" ] || { echo "CAMPAIGN_K4_TGZ is not set and no k4 raw next to the asset dir" >&2; exit 4; }
export CAMPAIGN_K4_TGZ
if [ "${2:-}" = "--build" ]; then export CAMPAIGN_REAL_BUILD=1; fi
cd "$here/.."
exec cargo test -p celeris-campaign --test real_asset_grade -- --nocapture \
  real_k2_assets_build_and_grade_known_bad_and_known_good real_k4_raw_recompute_matches_the_grader
