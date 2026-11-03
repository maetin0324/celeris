---
title: trace-read-fail — cos_digest の pass trace に read_run の失敗の場面を足す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# trace-read-fail — cos_digest の pass trace に read_run の失敗の場面を足す

## 結論
`control-ts/fixtures/cos_digest/pass_traces.jsonl` に、`cos_digest:read_run` が error（code `exception`）を返す pass が入った（20 行 → 22 行）。
この行は TS の replay（`compare.mjs`・control-ts の単体）と `script-host-runner` の golden 採点の両方で一致した。
境界 ADR 付記 S4-16 の場面の欠け（grade-cos-digest の記録の未解決 3）はこれで埋まった。挙動・評価器・安全検査は変えていない。

## 変更
- `phase s5: trace-read-fail: cos_digest_trace に read_run が失敗する場面を足す`
  - `crates/task-dispatch/src/dispatcher/tests/cos_digest_trace.rs`: `Recorder::record_with`（各 pass の前に DB を操作するフック）と場面 `read-failure/skip-then-digest` を足した。
    場面は store で直接作った 2 run（`read-ok` は completed、`read-fail` は stopped）を持つ。pass 1 の前に `read-fail` の item の summary を blob にする（report が読めず op の error `exception`）。pass 2 の前に戻す。
    mode `rust` と mode `ts` の書き込みの一致、`verify_writes` の照合、dispatcher の記録（`applied_by = ts`・`match`）は他の場面と同じ経路で確かめる。
  - 試験 `cos_digest_pass_trace_fixture_is_current` に「`read_run` が `exception` で失敗した run を飛ばし、同じ pass の別の run は digest した pass がある」検査を足した。run の状態・item の状態を数える集計は `reply.kind = ok` の read だけを見る。
- `phase s5: trace-read-fail: pass_traces.jsonl を書き直し、compare.mjs の注釈の旧名を消す`
  - `pass_traces.jsonl` は `CELERIS_UPDATE_FIXTURES=1` で書き直した（手書きなし）。足した行は次の 2 行。
    - `read-failure/skip-then-digest/pass-1`: list → read_run(`read-fail`) error `exception`（`sqlite error: Invalid column type Blob at index: 5, name: summary`）→ read_run(`read-ok`) ok。output は `read-ok` だけ。書き込みは message 1・requeue 0。
    - `read-failure/skip-then-digest/pass-2`: list → read_run(`read-fail`) ok。output は `read-fail`。書き込みは message 1・requeue 1。
  - `control-ts/scripts/compare.mjs` 10 行目の注釈から `traces.jsonl` を消した（`{ name, input, commands }` の形は `cases.jsonl` だけ。S4 の未解決 3）。
- `phase s5: trace-read-fail: cos_digest_trace の書式を cargo fmt に合わせる`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `CELERIS_UPDATE_FIXTURES=1 cargo nextest run -p task-dispatch cos_digest_pass_trace` → 4 passed。続けて環境変数なしで同じ試験 → 4 passed（byte 一致）。
- `node scripts/compare.mjs cos_digest`（control-ts で）→ `2088/2088 matched`、exit 0。`--fixture fixtures/cos_digest/pass_traces.jsonl` → `22/22 matched`。
- `corepack pnpm@12.6.0 -C control-ts test` → pass 44・fail 0。`run typecheck` → exit 0。
- `CELERIS_WU_BASE=<Rust の commit> bash scripts/dev/ts-verify.sh --grade` → exit 0（host-api の検証、段 golden を含む 10 段）。基準を Rust の commit にしたのは、Rust を含む差分では ts-verify が「従来の経路へ」となり golden 段を走らせないため。
- runner を直に `run --suite golden --domain cos_digest` → ok true、rows 2088、matched 2088、mismatch 0、exception 0（cache の golden の `pass_traces.jsonl` は 22 行で fixture と cmp 一致）。
- `bash scripts/dev/test-ts-verify.sh` → 18 passed, 0 failed、exit 0（場合 L は 1 行目の書き換えなので変更不要だった）。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 7111 passed・21 skipped、web 100 passed、control-ts の typecheck+test、ts-verify-grade、test-ts-verify 18 passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決
- `script-host-runner.rs` の `pass_traces.jsonl` の行数の下限（`min_rows`）は 20 のまま（`cases.jsonl` と合わせて 2086）。下限なので 22 行でも合格する。この unit の範囲外なので変えていない。

## 提案
- 下限を 22 に上げ、read_run の失敗の行が消えたら runner の採点でも落ちるようにする（今は cargo の `cos_digest_trace.rs` の検査だけが守る）。
