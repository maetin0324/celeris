---
title: S4 pass-wire — dispatcher の digest pass を TS の async pass へ結線し、mode ts で TS の command を検証→apply する
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 pass-wire — digest pass の TS の async 経路

完了日: 2026-11-02（unit pass-wire）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S4-12・S4-14・S4-15・S4-17（Rust の分）・S4-19 の pass-wire の行を実装した。

## 結論
- `PolicyScript::cos_digest`（run ごと）を消し、`PolicyScript::cos_digest_pass(CosDigestPassCall, deadline)` に置き換えた。`CosDigestPassCall` は `pass_id`（ULID）と op の送り口（容量 64 の bounded `mpsc`、応答は op ごとの `oneshot`）を持つ。
- dispatcher の digest pass（`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `Dispatcher::cos_digest_pass`）は、読み出しを pass の memo（`CosDigestPassReads`）に通す。list は 1 回、run ごとの read は 1 回まで store を読む。2 回目以降は memo が答える（op の log の `outcome = memo`）。
- mode は pass の始めに読む（tick ごとの `script_domain_modes` の読み出し）。
  - `rust`: Rust が memo で読み、`cos_digest_pass_decide` で決めて apply する。記録も TS の呼び出しも無い。
  - `compare`: `rust` と同じく apply した後、memo を凍結し、別の tokio task で TS の pass を 1 回走らせる。op は凍結した memo からだけ答える。memo に無い要求（list に無い run の read）は `denied`。記録だけ。
  - `ts`: TS の pass を先に走らせ、dispatcher は 200 ms（`COS_DIGEST_PASS_DEADLINE`）を上限に待つ。op は memo を通して store を読む。TS が読んでいない run は Rust が読み足し、同じ memo で Rust の参照実装の出力を作る。TS の出力を規則 1〜9 で検証し、apply の直前に mode の行を読み直す。通れば TS の command を、通らなければ pass 全体を Rust の command で apply する。
- fallback の code は `ts_error`（失敗・timeout）・`invalid_output`（形・規則 1〜6）・`run_outside_list`・`run_missing`・`run_unreadable`・`run_order`・`mode_changed`・`unavailable`。
- 規則 7〜9 は task-core の `validate_cos_digest_pass`。pass の DTO（`CosDigestListRunsRequest`・`CosDigestRunRef`・`CosDigestListRunsReply`・`CosDigestReadRunRequest`・`CosDigestPassRun`・`CosDigestPassOutput`）は `crates/task-core/src/chat/digest.rs`。
- `script_compare_log` の 1 行（`CosDigestCompareRecord`）に `mode`・`pass_id`・`applied_by`・`fallback`・`ops` を足した。`call_site` は `cos_digest.pass`。重複の抑えに `applied_by` を足した（`crates/task-core/src/store/script_domains.rs`）。
- 冪等 key・`digest_due`・`dirty`・20 件の上限・message と requeue の SQL は Rust のまま。apply は run ごとに `post_digest`→`requeue_run` で、書き込みが失敗した run は残りを書かず次の pass で回収する（fallback にしない）。
- daemon の実装（`crates/celeris/src/daemon/script_policy.rs`）は pass 1 回を `cos_digest` の要求 1 件として `call_with_host_sink` で送り、TS の op（`HostOpCall`）を dispatcher の op server へ写す。写しは呼び出しと同じ future の中で走る。入力は `{}`。
- 試験の古い dist の検出（`crates/celeris/tests/support/script_compare.rs` の `usable_bundle`）は、manifest の `capabilities` が `bundle.config.json` の各領域の capability の和集合と違う bundle も SKIP にする。

## 付記との違い・実装で決めたこと
- mode `ts` の待ち: tick は同期の関数なので、TS の pass と op server を、別 thread の current-thread runtime で 1 つの future として走らせる（`run_cluster_hooks_off_async` と同じ逃がし方。multi-thread runtime では `block_in_place`）。store は dispatcher 側に残り、V8 の thread には渡らない。
- `compare` の op の log の `outcome` は、凍結した memo から答えるので全部 `memo`（拒否は `denied`）。
- `denied` の op が 1 回でもあった pass では、記録の全行の `safety_violations` に `denied_scope` を入れる。op の `ops` 欄には、その run の read と list だけを入れる（list に無い run の read は、どの行の `ops` にも入らない）。
- 規則 1〜6 は run ごとに当て、その run の行を `invalid_output` にする。規則 7〜9 の違反のうち、run を名指さないもの・Rust の出力に無い run を名指すものは、全行を `invalid_output` にする。
- 記録は apply が全部通った run だけ書く（書き込みに失敗した run は次の pass で書く。既存の試験と同じ）。
- 試験の偽の TS の参照 pass は `crate::script_policy::test_pass`（`#[cfg(test)]`）。

## 試験
`crates/task-dispatch/src/dispatcher/tests/cos_digest_pass.rs`（S4-19 の名前のまま。`cos_digest_script.rs` の子 module）:
| 試験 | 確かめること |
|---|---|
| `cos_digest_pass_rust_reads_each_once_and_applies_rust` | mode rust は記録なしで 2 run を digest、requeue 1 回。memo は 2 回目の list・read を、表を改名した後でも store なしで答える（`ok, ok, memo, memo`） |
| `cos_digest_pass_compare_answers_ops_from_frozen_snapshot` | apply の後に表を改名しても TS の op は凍結した memo から答え、2 行とも `match`・`applied_by = rust`・ops は全部 `memo`。message・item は mode rust と byte 一致 |
| `cos_digest_pass_compare_denies_unlisted_read` | list に無い run の read は `denied`、記録に `denied_scope` |
| `cos_digest_pass_ts_applies_validated_ts_commands` | mode ts で TS の command を apply し、message・item・requeue が mode rust の基準と byte 一致、`applied_by = ts`。TS が文面を変えた（規則には通る）pass では、DB の message が TS の文面になる（TS の command を apply した証拠） |
| `cos_digest_pass_ts_falls_back_on_error_timeout_invalid` | 例外・timeout（200 ms）・形の誤り・規則 1（key）・規則 9（順序）・規則 8（欠け）で、message・item が基準と byte 一致、`applied_by = rust`、fallback の code |
| `cos_digest_pass_ts_rejects_run_outside_list` | 規則 7。`run_outside_list` の fallback と安全違反、知らない run の message は無い |
| `cos_digest_pass_ts_mode_changed_before_apply_uses_rust` | TS の返事の前に mode を rust に戻すと、TS の文面でなく Rust の command を apply（`mode_changed`） |
| `cos_digest_pass_log_records_applied_by_and_ops` | 同じ pass の行の `pass_id` が同じ、`call_site = cos_digest.pass`、ops は `list_runs`・その run の `read_run`（`ok`）、digest は `sha256:` |

task-core: `chat::digest::pass_tests::cos_digest_pass_validation_rejects_each_rule`（規則 1〜9）。`store::script_domains_tests::script_compare_log_appends_once_per_outcome_and_is_append_only` に `applied_by` の重複の抑えを足した。
既存 11 件（`cos_digest_script.rs`）・`cos_chat_triage_digest_script_compare.rs` は偽の TS を pass の形に直して通す（呼び出し回数は「run ごと」から「pass ごと」に変えた）。`crates/celeris/tests/script_compare_cos_digest.rs` は fixture の各行を run 1 つの pass に組み直して本物の host で比べる形に直した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -p task-core -E 'test(/cos_digest|cos_chat_triage|script_compare/)'`: 139 passed（exit 0）。
- `cargo nextest run -p celeris -E 'test(/script_compare/)'`: 19 passed（dist が無いので real-host の試験は SKIP で通る）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。task-core の変更で workspace 全体が対象になり、nextest 7075 passed・21 skipped、doc test も通った。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を計画の許可 path と照合）: 範囲外 0 件。

## 未解決事項
- 本物の bundle の `cosDigestPass` は ts-pass unit が書く。この unit の branch の `control-ts` は旧い `cosDigest` のままで、`bundle.config.json` の `cos_digest` の capability も空。本物の host での mode compare・ts の確認は real-host unit が行う。
- `traces.jsonl` と `cos_digest_trace.rs` は traces-async unit が消す（この unit では触っていない。旧い経路の API を使っていない）。
- `cargo clippy --workspace --all-targets` は `crates/task-core/src/chat/plan_rejudge_tests.rs:78` の既存の `cloned_ref_to_slice_refs` で落ちる（この unit の差分ではない。規定の check は `--all-targets` 無し）。

## 本番への移行
- 本番の `cos_digest` は `rust` のまま。`script_compare_log` の行の形が変わった（欄の追加）が、本番は mode rust で行を書いていない。新しい binary は旧い形の行（欄なし）を読めない。非本番の DB に compare の行があれば、読み出しの API が失敗する（未確認。本番では行が無い想定）。
