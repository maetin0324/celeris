---
title: S4 traces-async — 本物の dispatcher の digest pass から op の要求/応答と commands の trace を書き出し、TS の replay で一致を確かめる
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 traces-async — digest pass の trace（pass_traces.jsonl）

完了日: 2026-11-03（unit traces-async）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S4-16・S4-17（fixture と試験の分）・S4-19 の traces-async の行を実装した。

## 結論
- 新しい試験 `crates/task-dispatch/src/dispatcher/tests/cos_digest_pass_trace.rs`（`cos_chat_triage` の子 module `pass_trace`）が、各場面を本物の dispatcher の経路で走らせる。triage run は FakeAdapter の harness で終わり、CoS の判断は記録済みの値を store に書く。LLM は呼ばない。
- 同じ場面を mode `rust`（script 無し）と mode `ts` で 1 回ずつ走らせる。mode `ts` では、記録用の偽の TS（`TracePass`）が op の送り口越しに list と各 run の read を行う。dispatcher の pass の memo が返した要求と応答の組を、呼んだ順に記録する。
- trace の 1 行は 1 pass で、形は `{ name, ops, writes, output }`。
  - `ops`: op の要求と応答の組（`{ op, request, reply }`。付記 S4-16 の形）。
  - `output`: `ops` から組んだ読み出しに、Rust の参照実装 `cos_digest_pass_decide` を当てた出力。dispatcher の `script_compare_log` の行が `applied_by = ts`・`outcome = match`・`fallback = null` であることも確かめる。つまり dispatcher の中の Rust の判定と TS の出力が一致し、TS の command が apply された。
  - `writes`: pass の間に実際に書かれた digest message（thread・冪等 key・role・text・cards・run_id）と requeue（試験 DB の trigger が記録した `running`→`pending` の item）。
- `verify_writes` が `output` の commands と `writes` を照合する。照合する内容: `post_digest` ごとに message がちょうど 1 件あり、text・cards・run が一致する。`requeue_run` がある run では、read で `running` だった item がちょうど requeue されている。`requeue_run` が無い run の requeue は 0 件。commands で説明できない message・requeue は無い。
- mode `ts` の書き込みは、場面ごと・pass ごとに mode `rust` の書き込み（正規化後）と一致する。
- `CELERIS_UPDATE_FIXTURES=1` で `control-ts/fixtures/cos_digest/pass_traces.jsonl` を書き直す。この変数が無ければ、commit した file と生成結果の byte 一致を確かめる。時計は注入した試験の時計を使い、ULID は初出順の固定の id に置き換える。
- trace の op は `serde_json::Value` でなく型（`TraceOp`）で直列化する。serde_json の `preserve_order` は workspace の build の組み合わせで有無が変わり、`Value` のままだと欄の順、つまり byte 列が変わるからである。`-p task-dispatch` 単独の build と、`-p task-dispatch -p celeris`（deno_core 経由で `preserve_order` が有効）の build の両方で、byte 一致を確かめた。
- 旧い run ごとの trace（`control-ts/fixtures/cos_digest/traces.jsonl`・`crates/task-dispatch/src/dispatcher/tests/cos_digest_trace.rs`）は消した（S4-17）。`compare.mjs` の冒頭の注釈から `traces.jsonl` を除いた。

## 場面（20 pass）
- 旧 traces の 16 場面をすべて引き継いだ: completed 7（escalated-packet・escalated-option-details・running-unhandled・answered-observed-running・fallback-while-live・resolved-source-closed・handled-unknown-state）、failed 3、stopped 2、interrupted 3（人の割り込み）、completed/two-threads（1 pass に 2 thread）。
- 足した場面:
  - `stopped/two-running-requeued`: 1 つの `requeue_run` で 2 件を requeue する。
  - `stopped/run-unreadable-reason-null`: `chat_run_get` が失敗する（`usage_json` を usage でない JSON にする）。`reason: null` で digest する。
  - `drain/23-runs`: 終わった run が 23 件ある。pass 1 は上限の 20 件を処理し（requeue 10 件）、pass 2 は残りの 3 件を処理する。stopped・interrupted を混ぜている。
- `read_run` の失敗は別の試験 `cos_digest_pass_trace_read_failure_skips_the_run_in_both_modes` で確かめた。report の表（`cos_notification_routes`）を pass の間だけ改名すると、op は `exception` を返し、TS も Rust も run を飛ばして何も書かない。次の pass で digest される。この場面は fixture に入れていない（下の未解決 1）。

## 試験（S4-19 の名前との対応）
| S4-19 の名前 | 実装 |
|---|---|
| `cos_digest_pass_trace_fixture_is_current` | 同名 |
| （足した）照合が食い違いを見つける | `cos_digest_pass_trace_verify_detects_mismatch`（text・cards の違い、requeue の命令の有無と書き込みの食い違い、同じ key の 2 件目、命令の無い message、list に無い run） |
| （足した）read の失敗 | `cos_digest_pass_trace_read_failure_skips_the_run_in_both_modes` |
| （引き継ぎ）正規化 | `cos_digest_pass_trace_normalize_fixes_ids_in_first_seen_order` |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `CELERIS_UPDATE_FIXTURES=1 cargo nextest run -p task-dispatch -E 'test(/cos_digest_pass_trace/)'` → 4 passed（fixture を生成、20 行）。
- `cargo nextest run -p task-dispatch -E 'test(/cos_digest_pass_trace/)'`（変数なし）→ 4 passed（byte 一致）。`-p task-dispatch -p celeris` でも 4 passed。
- `node control-ts/scripts/compare.mjs cos_digest` → `2086/2086 matched (cases.jsonl, pass_traces.jsonl)`、exit 0。
- 写しの `pass_traces.jsonl` の `stopped/running-requeued` から `requeue_run` を消すと、`19/20 matched`・exit 1。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は下の「検査」に書く。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch で、nextest は 3028 passed・9 skipped。control-ts の typecheck と unit 試験（vitest 100 passed、`node --test` も通過）も走った。
  1 回目は文書の path 検査で落ちた: `docs/architecture-map.md` が消した `cos_digest_trace.rs` を指していた。その path を `cos_digest_pass_trace.rs` に直した（cos_digest の行の書き直し自体は close-async unit が行う）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-dispatch --tests -- -D warnings` も警告 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項
1. `read_run` が失敗する pass は fixture に入れていない。表の改名は report と同じ表を使う他の処理にも効くので、場面を固定の bytes にすると壊れやすい。TS 側の「`exception` の run を飛ばす」は `cos_digest.test.ts` の手書きの trace が確かめている。
2. trace の行に `writes` を足した。付記 S4-16 の形 `{ name, ops, output }` には無い欄である。`compare.mjs` は `output` が行の最後にあれば他の欄を読まないので、replay は変わらない。close-async unit が付記と突き合わせる。
3. `crates/celeris/tests/script_compare_cos_digest.rs` はまだ `traces.jsonl` を読む。dist の bundle が無いと SKIP するので、この unit の test-affected では落ちない。この file は real-host unit が `script_cos_digest_pass.rs` に置き換える（S4-19）。統合の時点で古い dist が残っていると落ちうる。

## 提案
- なし。
