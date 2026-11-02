---
title: S4 trace-capture — 記録済みの CoS run の trace を dispatcher の経路から書き出す
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 trace-capture — cos_digest の記録済み trace

完了日: 2026-11-02（unit trace-capture）。試験と fixture だけの変更。Rust の本番コードは変えていない。

## 結論
- 試験 file `crates/task-dispatch/src/dispatcher/tests/cos_digest_trace.rs` を足した。`cos_chat_triage.rs` の子 module（`#[path]`）にして、`cos_chat_triage_inbox_thread.rs` と同じ `Fixture`（偽 harness `sh -c`、注入した時計）を使う。LLM は呼ばない。CoS の出力は記録した値（escalation の packet と resolve）を store に書く。
- 各 scenario は triage run を本物の dispatcher の経路（`tick_cos_chat_launch`）で始めて終わらせる。digest pass を含む tick の直前に snapshot を読んで `cos_digest_decide` の command と組にし、`{"name","input","commands"}` の行にする。
- 生成物 `control-ts/fixtures/cos_digest/traces.jsonl`（17 行、16 scenario。2 thread の scenario だけ 2 行）。試験は生成した行と commit 済みの file の byte 一致を確かめる。`CELERIS_UPDATE_FIXTURES=1` のときだけ file を書き直す。
- digest pass の後で、書かれた message（thread・`client_message_id`・role assistant・text・cards・run_id）と item の行（requeue なら `pending`・`run_id` NULL、それ以外は run に残る）が commands と一致することを確かめる。2 回目の pass で何も増えないことも確かめる。
- 網羅: run_state `completed`・`failed`・`stopped`・`interrupted`。item state `escalated`・`fallback`・`running`・`handled`・`answered`・`observed`・`resolved`。item の種類は question と notice。project の CoS thread と general の thread の 2 run を 1 pass で digest する scenario を含む。

## 決定性
- 時刻: 注入した時計（tick ごとに 1 秒）。wait は 1 tick ずつずらして入れ（その間 `accepting_new_work=false`）、`created_at` で report の順序を決める。2 thread の run も別の tick で始めて `started_at` を分ける。
- ULID: 行ごとに出現順で `01TRACE<scenario 2 桁><連番 17 桁>` に置き換える（長さ 26 は変えない）。一時 directory の path は `/celeris-trace-data` にする。
- question の `source_revision` は task event の実時刻で、注入した時計が届かない。digest はこの欄を読まないので、item の `created_at` に置き換える。
- 置き換えた行を読み直し、`cos_digest_decide(input) == commands` と再直列化の一致を確かめる（TS 側の replay と同じ入力になる）。

## 本番コードを変えなかったことと seam
- `digest_snapshot` は `cos_chat::digest` の private 関数で、試験 module から呼べない。範囲 check も本番コードの変更を許さない。そこで試験側の `trace_snapshot` が同じ 3 つの store 呼び出し（`cos_triage_runs_without_digest(20)`・`cos_triage_run_report`・`chat_run_get(...).reason`）で `CosDigestInput` を組む。
  両者が食い違えば、pass の後の照合（text・cards・requeue）が落ちる。照合が食い違いを検出することは試験 `cos_digest_trace_verify_rejects_commands_that_differ_from_the_writes` で確かめた（text・cards・requeue の 3 通りの改変を拒む）。
- `handled` は `cos_inbox_items` の CHECK（`pending`・`running`・`answered`・`observed`・`escalated`・`fallback`・`resolved`）に無く、本番の経路では作れない。rust-split の cases fixture と同じく「digest が知らない state」として、1 行だけ `PRAGMA ignore_check_constraints` で書いた（scenario `completed/handled-unknown-state`、判断は「状態不明」）。CoS が処理した item の本物の state（`answered`・`observed`）は別の scenario で本物の経路から入れている。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（traces.jsonl が 10 行以上で、試験が再生成と byte 一致を確かめる）: `cargo nextest run -p task-dispatch -E 'test(cos_digest_trace)' && test "$(wc -l < control-ts/fixtures/cos_digest/traces.jsonl)" -ge 10` → 3 passed、17 行。`CELERIS_UPDATE_FIXTURES=1` で生成した file と、続けて生成なしで走らせた試験が一致した（`cmp` で差分なし。生成なしの実行を 4 回）。
- 条件 1（各 trace の commands が実際に書かれた message・requeue と一致する）: `trace_verify` が 17 run すべてで通る（同じ試験）。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 2,993 passed・9 skipped、control-ts 48 passed、progress_index_commit_hashes 合格）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-dispatch --tests -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更は `crates/task-dispatch/src/dispatcher/tests/` の 2 file、`control-ts/fixtures/cos_digest/traces.jsonl`、この記録だけ。

## commit（件名）
- phase s4: trace-capture: digest pass の記録済み trace を dispatcher の経路から書き出す試験と traces.jsonl を足す

## 未解決事項
- なし（この unit の範囲）。

## 提案
- `handled` は store では作れない state なので、close unit で付記 S4-8 の表の「item state」に「`handled` は CHECK 外の未知 state（試験だけで作る）」と書き添える。
- robustness unit は同じ `Fixture` と `trace_*` の手順（launch を止めて requeue を見る、2 thread を別の tick で始める）を使える。
