#!/bin/sh
# R5 v8-guard: cargo の target にある v8 の build script の出力（<profile>/build/v8-*/output）が
# 指す link-search の dir に librusty_v8.a があるかを確かめる。読むだけで、何も消さない・書かない。
#
# scratch の seed の写しから作った target では、v8 の build script の出力の
# cargo:rustc-link-search= が seed 世代の target/<profile>/gn_out/obj を絶対 path で指す。
# その世代が退避されると、features の違う cargo 呼び出しで v8 の lib を作り直す時に
# `could not find native static library rusty_v8` で落ちる（agent-docs/progress/2026-11-10-health-v2-r5/v8-guard.md）。
#
# 使い方: sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh（引数なし）
#   対象は $CARGO_TARGET_DIR（未設定なら target）。
#   exit 0: output が無い、または各 output の link-search の dir のどれかに librusty_v8.a がある
#   exit 1: librusty_v8.a がどの link-search の dir にも無い output がある（path・dir・直し方を出す）
#   exit 2: 引数を渡した
set -u

if [ "$#" -ne 0 ]; then
	echo "v8-target-check: 引数は取りません（対象は \$CARGO_TARGET_DIR、未設定なら target）" >&2
	exit 2
fi

target=${CARGO_TARGET_DIR:-target}
lib=librusty_v8.a
nl='
'
stale=0
checked=0

# 写し元の seed（scratch の owner の target-origin.json）。診断の表示だけに使う。
origin=
if [ -f "$target/../target-origin.json" ]; then
	origin=$(sed -n 's/^ *"seed": *"\(.*\)",\{0,1\} *$/\1/p' "$target/../target-origin.json" | head -n 1)
fi

for out in "$target"/*/build/v8-*/output; do
	[ -f "$out" ] || continue
	checked=$((checked + 1))
	# cargo:rustc-link-search=[KIND=]PATH と cargo::rustc-link-search=[KIND=]PATH を読む。
	dirs=$(sed -n 's/^cargo::\{0,1\}rustc-link-search=//p' "$out")
	[ -n "$dirs" ] || continue

	found=0
	set -f
	saved_ifs=$IFS
	IFS=$nl
	for d in $dirs; do
		case $d in
		native=* | all=* | dependency=* | crate=* | framework=*) d=${d#*=} ;;
		esac
		if [ -f "$d/$lib" ]; then
			found=1
		fi
	done
	IFS=$saved_ifs
	set +f
	[ "$found" -eq 1 ] && continue

	stale=1
	profile_dir=${out#"$target"/}
	profile_dir=${profile_dir%%/*}
	case $profile_dir in
	debug) clean_flag= ;;
	release) clean_flag=' --release' ;;
	*) clean_flag=" --profile $profile_dir" ;;
	esac
	echo "v8-target-check: NG stale v8 link-search"
	echo "  output: $out"
	set -f
	saved_ifs=$IFS
	IFS=$nl
	for d in $dirs; do
		case $d in
		native=* | all=* | dependency=* | crate=* | framework=*) d=${d#*=} ;;
		esac
		echo "  link-search: $d （$lib が無い）"
		case $d in
		*/seeds/*/gen-*)
			gen=${d%%/target/*}
			if [ -d "$gen" ]; then
				echo "  seed 世代: $gen （残っているが $lib が無い）"
			else
				echo "  seed 世代: $gen （退避済み）"
			fi
			;;
		esac
	done
	IFS=$saved_ifs
	set +f
	if [ -n "$origin" ]; then
		echo "  この target の写し元の seed: $origin"
	fi
	echo "  直し方 1: CARGO_TARGET_DIR=$target cargo clean -p v8$clean_flag （この target の v8 だけを作り直す。seed は触らない）"
	echo "  直し方 2: 原因は seed 世代の退避。scratch（crates/task-worker/src/scratch）が、生きている target の参照する世代を退避しないか、写すときに build/*/output の seed の prefix を owner の target に書き換える"
done

if [ "$stale" -ne 0 ]; then
	exit 1
fi
echo "v8-target-check: ok（$target の v8 の build output ${checked} 件）"
exit 0
