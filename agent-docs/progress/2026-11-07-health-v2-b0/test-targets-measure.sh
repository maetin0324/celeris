#!/usr/bin/env bash
# test-targets の時間計測（B0）。使い方: bash <this> build|edit <crate> <file> <out-dir>
#   build: cargo nextest run -p <crate> --no-run の時間と、生成された test binary 数
#   edit : <file> に 1 行足し、test-affected.sh --list（基準 = HEAD）の対象数と、
#          対象 crate 全部の cargo nextest run --no-run の再 build 時間を測り、編集を戻す
# 全 workspace 試験・並走負荷はしない。CARGO_TARGET_DIR は渡された値のまま使う。
set -u
mode="$1"; crate="$2"; file="${3:-}"; out="$4"
mkdir -p "$out"
now() { date +%s.%N; }
la() { cut -d' ' -f1-3 /proc/loadavg; }
j="$out/$mode-$crate.json"
if [ "$mode" = build ]; then
  la0="$(la)"; t0="$(now)"
  cargo nextest run -p "$crate" --no-run >"$out/$mode-$crate.log" 2>&1; rc=$?
  t1="$(now)"; la1="$(la)"
  cargo nextest list -p "$crate" --list-type binaries-only --message-format json 2>/dev/null >"$out/$mode-$crate.binaries.json"
  nb="$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["rust-binaries"]))' "$out/$mode-$crate.binaries.json" 2>/dev/null || echo null)"
  compiled="$(grep -c '^ *Compiling ' "$out/$mode-$crate.log")"
  python3 - "$j" <<P
import json,sys
json.dump({"mode":"build","crate":"$crate","exit":$rc,"secs":round($t1-$t0,2),"test_binaries":$nb,
 "compiled_units":$compiled,"loadavg_before":"$la0","loadavg_after":"$la1"},open(sys.argv[1],"w"),indent=1)
P
else
  printf '\n// edit-verify probe (B0 test-targets)\n' >>"$file"
  la0="$(la)"; t0="$(now)"
  CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list >"$out/$mode-$crate.list" 2>"$out/$mode-$crate.list.err"; lrc=$?
  t1="$(now)"
  args=(); for c in $(grep -v ':' "$out/$mode-$crate.list"); do args+=(-p "$c"); done
  t2="$(now)"
  cargo nextest run "${args[@]}" --no-run >"$out/$mode-$crate.log" 2>&1; rc=$?
  t3="$(now)"; la1="$(la)"
  git checkout -- "$file"
  n="$(grep -vc ':' "$out/$mode-$crate.list")"
  compiled="$(grep -c '^ *Compiling ' "$out/$mode-$crate.log")"
  python3 - "$j" <<P
import json,sys
json.dump({"mode":"edit","crate":"$crate","edited_file":"$file","list_exit":$lrc,"list_secs":round($t1-$t0,2),
 "affected_crates":$n,"affected":[l for l in open("$out/$mode-$crate.list").read().splitlines() if ":" not in l],"extra_checks":[l for l in open("$out/$mode-$crate.list").read().splitlines() if ":" in l],"rebuild_exit":$rc,"rebuild_secs":round($t3-$t2,2),
 "edit_to_verify_secs":round($t3-$t0,2),"compiled_units":$compiled,"loadavg_before":"$la0","loadavg_after":"$la1"},open(sys.argv[1],"w"),indent=1)
P
fi
cat "$j"
