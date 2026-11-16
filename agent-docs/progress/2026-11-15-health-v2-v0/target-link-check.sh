#!/bin/sh
# target-link-check.sh — cargo の target の build script 出力のうち、cargo が付け替えない
# rustc-link-search が消えた dir を指していないかを確かめる（健全性v2 V0 target-env）。
#
# 使い方:
#   sh target-link-check.sh <target dir>   # MISSING があれば exit 1、無ければ exit 0
#   sh target-link-check.sh --self-test    # $TMPDIR に偽の target を作り 3 例を確かめる
#
# cargo は target を別の場所に写した後、build script の出力 output を読むとき、root-output に
# 記録した前の OUT_DIR の文字列だけを今の OUT_DIR に付け替える。OUT_DIR の外を指す値
# （v8 の target/debug/gn_out/obj など）は写す前の path のまま残る。その dir が消えると、
# build script が再実行されない限り link が失敗する（could not find native static library）。
#
# 判定: <target dir>/*/build/*/output の rustc-link-search の値（native= などの前置きを外す）が
#   - 同じ dir の root-output の path（無ければ今の OUT_DIR）かその下 → cargo が付け替えるので数えるだけ
#   - それ以外で dir が在る → 数えるだけ
#   - それ以外で dir が無い → MISSING <path> <- <output>
# exit: 0 = MISSING 0 件、1 = MISSING あり、2 = 引数の誤り。
set -eu

usage() {
	echo "usage: sh $0 <target dir> | --self-test" >&2
	exit 2
}

# check_target <target dir>: 結果を stdout に出し、MISSING があれば 1 を返す。
check_target() {
	target=$1
	outputs=0
	searches=0
	under_root=0
	present=0
	missing=0
	for output in "$target"/*/build/*/output; do
		[ -f "$output" ] || continue
		outputs=$((outputs + 1))
		build_dir=$(dirname "$output")
		if [ -f "$build_dir/root-output" ]; then
			root=$(cat "$build_dir/root-output")
		else
			root=$build_dir/out
		fi
		while IFS= read -r line || [ -n "$line" ]; do
			case $line in
			cargo::rustc-link-search=*) value=${line#cargo::rustc-link-search=} ;;
			cargo:rustc-link-search=*) value=${line#cargo:rustc-link-search=} ;;
			*) continue ;;
			esac
			case $value in
			native=* | dependency=* | crate=* | framework=* | all=*) value=${value#*=} ;;
			esac
			searches=$((searches + 1))
			case $value in
			"$root" | "$root"/*)
				under_root=$((under_root + 1))
				continue
				;;
			esac
			if [ -d "$value" ]; then
				present=$((present + 1))
			else
				missing=$((missing + 1))
				echo "MISSING $value <- $output"
			fi
		done <"$output"
	done
	echo "target-link-check: $target: outputs=$outputs link-search=$searches under-root-output=$under_root outside-present=$present missing=$missing"
	[ "$missing" -eq 0 ]
}

# fake_output <target> <name> <link-search value>: 偽の build script 出力を 1 つ作る。
fake_output() {
	dir=$1/debug/build/$2
	mkdir -p "$dir/out"
	# root-output は写す前の OUT_DIR を指す（cargo が付け替える元の文字列）。
	printf '%s\n' "$1-before-copy/debug/build/$2/out" >"$dir/root-output"
	printf 'cargo:rustc-link-lib=static=fake\ncargo:rustc-link-search=%s\n' "$3" >"$dir/output"
}

self_test() {
	work=$(mktemp -d "${TMPDIR:-/tmp}/target-link-check.XXXXXX")
	trap 'rm -rf "$work"' EXIT HUP INT TERM
	target=$work/target
	before=$target-before-copy
	mkdir -p "$work/outside-present"
	failed=0

	# 例 1: root-output の下（dir は無いが cargo が付け替える）→ MISSING にしない。
	fake_output "$target" under-root-0000 "native=$before/debug/build/under-root-0000/out/lib"
	# 例 2: root-output の外で dir が在る → MISSING にしない。
	fake_output "$target" outside-present-0000 "$work/outside-present"
	if out=$(check_target "$target"); then
		case $out in
		*"link-search=2 under-root-output=1 outside-present=1 missing=0"*) echo "self-test ok: under root-output and outside-present are not MISSING (exit 0)" ;;
		*)
			echo "self-test FAILED: unexpected counts: $out" >&2
			failed=1
			;;
		esac
	else
		echo "self-test FAILED: cases 1-2 gave exit 1: $out" >&2
		failed=1
	fi

	# 例 3: root-output の外で dir が無い → MISSING と出して exit 1。
	fake_output "$target" outside-missing-0000 "$before/debug/gn_out/obj"
	if out=$(check_target "$target"); then
		echo "self-test FAILED: case 3 gave exit 0: $out" >&2
		failed=1
	else
		expected="MISSING $before/debug/gn_out/obj <- $target/debug/build/outside-missing-0000/output"
		case $out in
		*"$expected"*"missing=1"*) echo "self-test ok: outside-missing is reported as MISSING (exit 1)" ;;
		*)
			echo "self-test FAILED: case 3 output lacks '$expected': $out" >&2
			failed=1
			;;
		esac
	fi

	# 引数が空なら exit 2。
	set +e
	sh "$0" "" 2>/dev/null
	code=$?
	set -e
	if [ "$code" -eq 2 ]; then
		echo "self-test ok: empty argument gives exit 2"
	else
		echo "self-test FAILED: empty argument gave exit $code" >&2
		failed=1
	fi

	if [ "$failed" -ne 0 ]; then
		exit 1
	fi
	echo "self-test: 3 cases ok"
}

[ $# -eq 1 ] || usage
case $1 in
"") usage ;;
--self-test) self_test ;;
*)
	[ -d "$1" ] || {
		echo "target-link-check: not a directory: $1" >&2
		exit 2
	}
	check_target "$1"
	;;
esac
