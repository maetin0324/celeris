---
title: S2 dispatch-hook — dispatch_run の lane 判定に compare の呼び出しを付ける（trait PolicyScript）
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 dispatch-hook

## 結論
- `crates/task-dispatch/src/script_policy.rs` に TS 制御層の判定の口を作った。deno_core には依存しない。
  - `trait PolicyScript`: `bundle()`（`bundle_sha256`・`host_api`）と `lane_rules(LaneRulesInput, deadline) -> PolicyFuture`。入出力は DTO（`LaneRulesInput` と `serde_json::Value`）だけ。
  - `LaneRulesOutput { rule_id, proposed, condition }`（`deny_unknown_fields`、付記 S2-2 の出力 DTO）。
  - `classify_lane_rules`: TS の結果を出力 DTO に deserialize し直し、Rust と同じ直列化の byte 列で比べる純粋関数。正規化はしない。outcome（`match`・`mismatch`・`ts_error`・`invalid_output`）と、`ScriptError::Denied` の `denied_capability`・`denied_scope`、`ceiling_exceeded`（S2-8）を決める。`rule_id` が空か 128 byte 超え、`condition` が 512 byte 超えなら `invalid_output`。
  - `call_lane_rules`: dispatcher 側でも `tokio::time::timeout(SCRIPT_CALL_DEADLINE = 50 ms)` で打ち切る。実装が deadline を守らなくても 50 ms より長く待たない。
  - `trait ScriptDomainStore`（`SqliteStore` に実装）: mode の表・generation・記録の追記の口。`TaskStore`（task-core）に欄を足さないため、この crate に置いた。
- dispatcher（`crates/task-dispatch/src/dispatcher/script_compare.rs`、`dispatch_run.rs` から `#[path]` で宣言）:
  - 注入は `Dispatcher::set_script_domain_store(Arc<dyn ScriptDomainStore>)` と `Dispatcher::set_policy_script(Option<Arc<dyn PolicyScript>>)`。どちらも渡さなければ compare は働かず、今までと同じ。
  - mode は tick ごとに 1 回だけ読む。その tick で最初に `decide_lane` が比べようとした時に読む。読めなければ、その tick は `rust` と扱う。
  - `decide_lane` の `decide_for_task` の直後に、`decision.source.policy_decides()`（`Hint`・`Default`）のときだけ `compare_lane_rules` を呼ぶ。人の明示・System の task では TS を呼ばない。比べる lane は天井で丸めた後、retry の escalation の前の lane。
  - mode が `compare` でなければ TS を呼ばない。`compare` なら次のように扱う。
    - generation が無い・`loaded` でない・`lane_rules` を宣言していない、`PolicyScript` が無い、bundle の hash が generation と違う、tokio の runtime が無い: どの場合も TS を呼ばず、`unavailable` を記録する（`error.kind = "unavailable"`、理由は `error.message`）。
    - それ以外: tokio の task で TS を呼び、結果を `script_compare_recorded` として追記する。dispatcher は TS を待たない。走っている compare は最大 32 件で、超えた判定は比べない。
  - `Dispatcher::wait_script_compares()` は走っている compare を待つ（試験と停止前の片付け用）。
  - `run_id` は `null`。`decide_lane` は run の id を作る前に走る（付記 S2-4 の「作る前なら null」）。
  - 記録の追記に失敗しても warn の log だけを出して dispatch を続ける。
- `work_units.rs` の `decide_lane_for_work_unit` の呼び出しは変えていない（WU の run では TS を呼ばない）。
- 挙動を変えない移動: `DispatchRoutingSettings` を `dispatcher.rs` から `dispatch_run.rs` へ移し、`dispatcher.rs` から `pub use` した。source-size gate（`dispatcher.rs` は比較基準より大きくできない）に通すため。型・公開 path（`task_dispatch::DispatchRoutingSettings`）は変わらない。
- dev-dependencies に tokio の `test-util` を足した（`start_paused`）。`Cargo.lock` は変わらない。

## 付記 S2-6 との違い
- 付記は「dispatcher が 1 判定で待つ時間は最大 50 ms」としている。実装は 0 ms（待たない）にした。理由: `tick()` は同期の関数で、tokio の task の中から呼ばれる。待つには `block_in_place` が要るが、`current_thread` の runtime では panic する。待たないので、Rust の結果と dispatch の速さは compare の有無で変わらない。記録は少し遅れて追記される。close unit で付記に反映する（下の提案 1）。

## 試験（名前に script_compare を含む）
dispatcher（`crates/task-dispatch/src/dispatcher/tests/script_compare.rs`）。偽の `PolicyScript` だけを使う。どの試験も、`routing_decided` の `LaneDecision` が mode rust・PolicyScript 無しの dispatch（基準）と等しいことを確かめる。
- `script_compare_match_records_and_keeps_rust_lane`: 一致すると `match` を 1 件記録する。記録した入力は 9 軸の features だけ。
- `script_compare_mismatch_records_and_keeps_rust_lane`: 常に frontier を返す TS では `mismatch` を記録し、lane は Rust のまま。
- `script_compare_ts_error_and_invalid_output_keep_rust_lane`: 例外・未知の enum 値・余計な欄・`null` で、`ts_error`・`invalid_output` を記録し、lane は Rust のまま。
- `script_compare_timeout_records_ts_error_and_keeps_rust_lane`（`start_paused`）: 返らない TS は 50 ms で `ts_error`（`timeout`）になる。`elapsed_us` は 50,000 以上 100,000 未満。
- `script_compare_mode_rust_never_calls_ts`: mode rust では呼び出し回数 0、記録 0。
- `script_compare_without_policy_script_records_unavailable`・`script_compare_other_bundle_is_unavailable_and_not_called`。
- `script_compare_human_tier_never_calls_ts`: 人の明示の task では呼び出し回数 0、lane は人が決めた cheap。
- `script_compare_lane_after_ceiling_is_rust`: 担当の `max_lane = standard` で、Rust の提案 frontier が standard に丸まる。TS が cheap を返しても frontier を返しても、lane は standard のまま。
単体（`crates/task-dispatch/src/script_policy_tests.rs`）:
- `script_compare_classify_match_needs_all_three_fields_byte_equal`・`script_compare_classify_key_order_is_not_a_difference`・`script_compare_classify_invalid_outputs`・`script_compare_classify_errors_and_denied_safety_violation`・`script_compare_draft_records_redacted_input_and_rust_output`・`script_compare_call_stops_waiting_at_deadline`。

## commit（件名）
1. `phase s2: dispatch-hook: decide_lane の lane_rules に compare を付ける（trait PolicyScript）`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(script_compare)'` — exit 0、15 passed。
- `bash scripts/dev/test-affected.sh` — exit 0。source-size gate は 0 failure。cargo 2952 passed・9 skipped（対象は task-dispatch・celeris・celeris-campaign・celerisctl・llm-proxy）。
- `cargo clippy --workspace -- -D warnings` — exit 0。`cargo clippy -p task-dispatch --tests -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。
- 範囲 check（計画のもの）— exit 0、範囲外の path なし。

## 未解決
1. `ceiling_exceeded` は付記 S2-8 の定義どおりに実装したが、今の `LaneCeiling::clamp` では起きない。許される lane が 1 つでもあれば `clamp` は許される lane を返す。1 つも無ければ Rust の lane も許されない。このため試験は「違反なし」だけを確かめている。
2. `LaneRulesInput`・`LaneRulesOutput` を `docs/api/script-host/host-v1.schema.json` に入れて TS の型を生成する作業（lane-rules-ts の未解決）は残っている。schema の生成元は `crates/script-runtime` で、この unit の範囲外。
3. 本物の host での実装（`ScriptRuntimeHandle` の `ScriptError` → `PolicyScriptError` の写像。`Denied` の code は `safety_violation` に入れる）と daemon の注入は host-adapter unit が行う。daemon は `[script]` の有無によらず `set_script_domain_store` を呼ぶ必要がある。呼ばないと、mode が compare でも `unavailable` が記録されない。

## 提案
1. close unit で付記 S2-6 の「最大 50 ms 待つ」を「dispatcher は待たず、比較は別の tokio task で 50 ms の deadline 付きで走る」に直す。
2. `ScriptDomainStore` の 3 つの method は、将来 `TaskStore` に移して 1 つの口にまとめてよい（task-core の変更なので別の unit で行う）。
