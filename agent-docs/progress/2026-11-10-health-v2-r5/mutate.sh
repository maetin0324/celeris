#!/bin/sh
# 健全性v2 R5 guard: 限定 mutation を 1 つ当てて、残した試験がそれを検出するかを見る。
#
#   sh agent-docs/progress/2026-11-10-health-v2-r5/mutate.sh <patch> <crate> <nextest filter>
#
# patch を git apply し、`cargo nextest run -p <crate> -E <filter>` を走らせて、必ず git apply -R で戻す
# （正常終了・失敗・INT/TERM/HUP の中断のどれでも。戻した後に patch の対象 file の内容が当てる前と同じかを確かめる）。
#
# 結果と exit code（cargo-nextest 0.9.146 で確かめた exit code: 通過 0・試験の失敗 100・build の失敗 101・
# 試験が 0 件 4・filter の誤り 94）:
#   KILLED   0  nextest が試験の失敗（exit 100）で終わった。mutation を検出した
#   SURVIVED 1  nextest が通った（exit 0）。mutation を検出しなかった
#   ERROR    2  引数・patch・build の失敗・試験 0 件・filter の誤りなど。検出力の根拠にならない
#               （compile しない mutation を KILLED と数えない）
#   ERROR    3  patch を戻せなかった、または戻した後の内容が当てる前と違う（作業ツリーを手で確かめる）
#   中断       INT 130 / TERM 143 / HUP 129（戻してから終わる。戻せなければ 3）
set -u

usage() {
	echo "usage: sh mutate.sh <patch> <crate> <nextest filter>" >&2
	exit 2
}
[ $# -eq 3 ] || usage
patch=$1
crate=$2
filter=$3
[ -n "$crate" ] && [ -n "$filter" ] || usage
[ -f "$patch" ] || {
	echo "mutate: ERROR patch が無い: $patch" >&2
	exit 2
}
case $patch in
/*) ;;
*) patch=$(pwd)/$patch ;;
esac
top=$(git rev-parse --show-toplevel) || exit 2
cd "$top" || exit 2

# patch の対象 file と、当てる前の内容の hash（無い file は absent）
fingerprint() {
	git apply --numstat "$patch" | while IFS="$(printf '\t')" read -r _add _del path; do
		if [ -f "$path" ]; then
			printf '%s %s\n' "$path" "$(git hash-object -- "$path")"
		else
			printf '%s absent\n' "$path"
		fi
	done
}

if ! git apply --check "$patch"; then
	echo "mutate: ERROR patch が当たらない: $patch" >&2
	exit 2
fi
before=$(fingerprint) || exit 2

applied=0
child=
restore() {
	if [ -n "$child" ]; then
		kill -TERM "$child" 2>/dev/null
		wait "$child" 2>/dev/null
		child=
	fi
	if [ "$applied" = 1 ]; then
		if git apply -R "$patch"; then
			applied=0
			after=$(fingerprint)
			if [ "$after" != "$before" ]; then
				echo "mutate: ERROR 戻した後の内容が当てる前と違う: $patch" >&2
				applied=2
			fi
		else
			echo "mutate: ERROR patch を戻せない: $patch（作業ツリーを手で確かめる）" >&2
		fi
	fi
}
on_exit() {
	rc=$?
	trap - EXIT INT TERM HUP
	restore
	[ "$applied" = 0 ] || rc=3
	exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

git apply "$patch" || {
	echo "mutate: ERROR patch を当てられない: $patch" >&2
	exit 2
}
applied=1
echo "mutate: applied $patch; cargo nextest run -p $crate -E '$filter'" >&2

# 背景で走らせて wait する（wait は trap した signal で抜けるので、中断を待たずに戻せる）
cargo nextest run -p "$crate" -E "$filter" &
child=$!
wait "$child"
rc=$?
child=
restore
if [ "$applied" != 0 ]; then
	exit 3
fi
case $rc in
100)
	echo "KILLED: $patch（nextest exit 100）"
	exit 0
	;;
0)
	echo "SURVIVED: $patch（nextest exit 0）"
	exit 1
	;;
*)
	echo "ERROR: $patch（nextest exit $rc。build の失敗・試験 0 件・filter の誤りなどは検出の根拠にしない）"
	exit 2
	;;
esac
