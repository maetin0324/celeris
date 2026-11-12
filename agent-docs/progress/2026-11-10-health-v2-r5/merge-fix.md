---
title: R5 merge-fix — main の取り込みで壊れた knowledge.rs の試験の build を直す
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# R5 merge-fix — main の取り込みで壊れた knowledge.rs の試験の build を直す

完了日: 2026-11-12。基準は `$CELERIS_WU_BASE`（task としては `git merge-base HEAD main`）。

## 原因
- review 前の同期で main 71f9ae2e を task branch に merge した（commit「Merge commit '71f9ae2e…' into celeris/01M6TYVJT53598CPKMGSQGA7XM」）。
- main は `crates/task-api/tests/knowledge.rs` に試験 `the_cos_memory_note_route_records_and_accepts_into_the_memory_page` を足した。この試験は file 内の helper `env()`・`p()` を呼ぶ。
- R5 の dedupe-api は同じ file の `env()`・`p()`・`g()`・`pu()` を消し、`tests/common` の `admin_env()`・`post_admin()`・`get_admin()`・`put_admin()` に寄せていた。merge は衝突せず、新しい試験だけが消えた helper を呼ぶ形で残った。
- その結果、`cargo test --no-run --workspace` は `task-api`（test "knowledge"）の E0423・E0425 の 3 件で exit 101 になった。

## 変更と commit（件名）
- 「phase 1: merge-fix: knowledge.rs の main の試験を tests/common の admin_env・post_admin に合わせる」: `crates/task-api/tests/knowledge.rs` の `the_cos_memory_note_route_records_and_accepts_into_the_memory_page` で `env()` を `admin_env()` に、`p(…)` の 2 か所を `post_admin(…)` に替えた。要求の path・body・assert は変えていない。消した helper は file に戻さず、`tests/common` に別名・互換の関数を足していない。
- 「phase 1: merge-fix: 原因と検査の結果を進捗に書く」: この file と R5 の進捗の leaf の表・commit の一覧。
- 他の file の build の失敗は無かった（下の `cargo test --no-run`）。merge で入った main の試験のうち R5 が束ねた・寄せた file（`cos_delivery_suite_a/cos_job_wait_extend.rs`・`cos_delivery_suite_c/single_human_window.rs`・`crates/celeris/tests/instance_handoff.rs`・`tests/support/script_compare.rs`）は直す所が無かった。
- 検査器・baseline・test-affected の対象規則は変えていない。test target の数は変わっていない（既存の target の中の 1 試験の書き換え）。

## 証拠（提出時の HEAD で実行）
- `cargo test --no-run --workspace --no-fail-fast` → exit 0、270 秒、test の実行 file 312 件、error 0。（`--keep-going` は `cargo test` が受け付けないので、build の失敗でも残りを build する `--no-fail-fast` を使った。）warning は main と同じ内容の `crates/celeris/tests/script_exhaustive.rs` の unused import `ScriptGenerationControl` などで、R5 の変更が原因ではない。
- `grep -nE "^\s*fn (env|p|g|pu)\(" crates/task-api/tests/knowledge.rs` → 該当 0 件（exit 1）。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$(git merge-base HEAD main)"` → exit 0、試験関数 858 → 858、表の行 761 → 761、名前で追った移動 326、不変条件の試験 278（42 file）、removals 0。celeris（809 → 808、removals 1）・celerisctl（205 → 205）・task-worker（1298 → 1298）も OK。
- `bash scripts/dev/test-affected.sh` → exit 0、783 秒（開始 load 78、終了 load 21）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web（typecheck+unit）。nextest 5961 件 passed・22 skipped・失敗 0、web の vitest 103 file・711 件 passed、repo health pass。負荷で落ちた範囲外の試験は無かった。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff main -- crates/task-api/tests/knowledge.rs` で新しい試験の定義は 1 回だけ。`crates/celeris/tests/support/script_compare.rs` の `[selfdeploy]` の表も 1 つだけで、main と同じ直しは重なっていない。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は `crates/task-api/tests/knowledge.rs` と R5 の進捗 2 file。

## 未解決事項
- 提出時の main は 71f9ae2e より進んでいる。71f9ae2e から main の先頭までに `crates/{task-api,celeris,celerisctl,task-worker}/tests/` の変更は無い（`git diff 71f9ae2e main --stat -- <4 dir>` が空）。次の同期でこの 4 dir の試験が同じ形で壊れることは今の main からは起きない。
