#!/bin/sh
# grader 側の挿入 script。隠しテストを子 module として作業 dir の src に足してから、引数の評価 command を走らせる。
# 成果物の src が何を書き換えていても末尾へ足すだけなので、成果物側に隠しテストが無いことは問題にしない。
set -eu
add() { printf '\n#[cfg(test)]\n#[path = "%s"]\nmod %s;\n' "$1" "$2" >> "$3"; }
add ../../.grader/k1_data_ops_hidden.rs k1_hidden_tests src/rpc/data_ops.rs
add ../../.grader/k1_chunk_store_hidden.rs k1_hidden_tests src/storage/chunk_store.rs
exec "$@"
