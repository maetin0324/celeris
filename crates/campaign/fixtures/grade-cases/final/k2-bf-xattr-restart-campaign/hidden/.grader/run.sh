#!/bin/sh
# grader 側の挿入 script（K1 と同じ。src/rpc/metadata_ops.rs の末尾に隠しテスト module を足す）。
set -eu
add() { printf '\n#[cfg(test)]\n#[path = "%s"]\nmod %s;\n' "$1" "$2" >> "$3"; }
add ../../.grader/k2_metadata_ops_hidden.rs k2_hidden_tests src/rpc/metadata_ops.rs
exec "$@"
