---
title: runtime-gen — script-runtime に generation の登録簿（新 isolate で検証・固定・drain・GenerationGone）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# runtime-gen — script-runtime に generation の登録簿

## 結論
`crates/script-runtime/src/generation.rs` に `GenerationRegistry` を作った（境界 ADR の付記 S5-1・S5-2・S5-5）。
daemon・store・API への配線はこの unit では行っていない（daemon-gen・store-gen・api-canary の範囲）。

- `register(id, VerifiedBundle)`・`register_deployed(id, state_dir, sha)`: 新しい actor（新しい isolate）で bundle を評価し、次を検証してから登録する。
  - manifest: 既知の領域だけ（`KNOWN_DOMAINS`）、capability ⊆ 宣言した領域の許可表の和、`checkpoint_versions` が重複なしの昇順で host の版（`[1]`）の中、`host_selftest` の宣言。
  - isolate の中: selftest（`selftest::check_output`）と、manifest の各領域の入口の確認。
  - `register_deployed` は配備先の名前と hash の照合（`load_deployed`）も行う。
  - 落ちたら isolate を捨て、登録簿に入れない。上限の枠も残さない。
- `call(id, call, sink)`: 呼んだ時点（future を作った時点）で generation の `Arc` と in-flight の枠を取る。
  - 切り替えの後も、その要求は取った generation で最後まで走る。返り値 `PinnedReply` に固定した generation の id が入る。
  - 未登録・drain 中・退役した generation への要求は即 `GenerationGone`。in-flight が 64 なら `Busy`。
- `drain(id)` → `sweep()`: in-flight が 0 になった generation を退役させる（`Drained`）。
  - drain から 30 秒（`DRAIN_CUTOFF`、注入した時計で数える）を過ぎて残る要求は打ち切り、`GenerationGone` を返す（`Cutoff`、`cancelled_in_flight` に件数）。
  - actor は後から来た結果を捨てる。同期の実行は要求の deadline で watchdog が止める（今の actor のまま）。
- 生きた generation（drain 中を含む）は 2 つまで。3 つ目の登録は `GenerationError::Limit`。
- `retire(id)` は in-flight の無い generation をすぐ退役させる。in-flight があれば `InFlight` を返す。`invalidate(id)` は残った要求を即座に打ち切る（S5-7 の安全違反用）。
- `checkpoint_versions_cover(stored, manifest_versions)` は付記 S5-5 の照合。足りない版を `CheckpointIncompatible { missing }` で返す。
- 時計は trait `GenerationClock`。既定は `TokioClock`（`tokio::time::pause` に従う）、試験は `ManualClock` を使う。
- Promise・closure は generation の間で移さない。isolate を跨ぐ値は JSON の DTO だけ。

## 設計の判断
- 要求の固定と in-flight の枠は、`call` の同期部分で registry の lock の中で取る。drain・sweep も同じ lock で判定するので、「drain の後に枠を取る要求」と「in-flight 0 と見て退役」は食い違わない。
- 打ち切りは generation ごとの `watch::Sender<bool>` で行う。要求の future は `biased` の select で、打ち切りの合図を actor の返事より先に見る。actor の shutdown（`Terminated`）より `GenerationGone` が優先される。
- 入口の確認は、入力 `null`・scope 空・host sink 無しで各領域を 1 回呼び、入口が `unknown domain: <領域>` で拒んだら「入口が無い」とする。
  - host sink が無いので op は副作用を持てない。
  - bundle の入口の文面（`control-ts/src/main.ts`）に頼る判定である（未解決 1）。
- `ScriptRuntimeHandle`・`actor.rs`・`watchdog.rs` は変えていない。今の handle の「generation」（restart の番号）は isolate の中の作り直しの番号のまま。付記 S5 の generation は登録簿の id（DB の `script_generations.id`）である。

## 試験（付記 S5-12 の名前との対応）
`crates/script-runtime/tests/generation.rs`。実時間の sleep・CPU を使う待ちは使わない。in-flight は「解決しない Promise」か「試験が答えるまで待つ host op」で作る。
| S5-12 の名前 | 実装 | 確かめること |
|---|---|---|
| `generation_load_uses_new_isolate` | 同名 | module の状態が generation の間で共有されない |
| `generation_load_rejects_unknown_domain_capability_or_checkpoint` | 同名 | 未知の領域・領域外の capability・未知の checkpoint の版・昇順でない版・selftest 無し・入口の無い領域を登録しない。枠を残さない |
| `generation_call_pins_requested_generation` | 同名 | 切り替えの前の要求は旧 generation で完了し、後の要求は新 generation へ行く |
| `generation_limit_two_live` | 同名 | 3 つ目を拒む。drain 中も数える。in-flight があると `retire` は `InFlight` |
| `generation_drain_cuts_pure_call_at_30s` | 同名（`ManualClock`） | 29.999 秒では打ち切らず、30 秒で `GenerationGone`・`cancelled_in_flight` 1 |
| `generation_call_to_retired_is_generation_gone` | 同名 | 退役・未登録は `GenerationGone`。同じ bundle を新しい isolate で登録し直せる |
| `generation_in_flight_64_then_busy` | 同名 | 65 件目は `Busy`。他の generation の枠は別に数える |
| `checkpoint_versions_must_cover_stored` | 同名 | 作り物の V で照合 |
| （追加）`generation_drain_completes_when_in_flight_reaches_zero` | — | drain の完了（`Drained`、打ち切り 0） |
| （追加）`generation_register_deployed_checks_name_and_hash` | — | 配備先の照合 |
| （追加）`generation_registers_real_bundle` | — | 本物の bundle が検証に通る。`CELERIS_SCRIPT_BUNDLE_DIR` が無ければ SKIP し、`CELERIS_SCRIPT_SKIP_LOG` に 1 行足す（付記 S5-9） |

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris-script --test generation`: 11 件が通過（本物の bundle の 1 件は SKIP の分岐で通過）。3 回続けて実行し、3 回とも全件が通過した。
- `CELERIS_SCRIPT_BUNDLE_DIR=$TMPDIR/realbundle/control-ts/dist cargo nextest run -p celeris-script --test generation generation_registers_real`: 通過。bundle は `control-ts` の写しを `$TMPDIR` で `pnpm install --offline` してから `build-bundle.mjs` で作った。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-script・celerisctl と ts-verify の test-ts-verify。959 件が通過、SKIP 0。
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `crates/script-runtime/src/{generation.rs,lib.rs}`、`crates/script-runtime/tests/generation.rs` とこの進捗ファイルだけ

## 未解決
1. 入口の確認は bundle の入口の文面 `unknown domain: <領域>` に頼る。`main.ts` の文面を変えると確認が素通りになる。bundle が領域の一覧を export する形（例 `globalThis.__celerisDomains`）にすれば文面に頼らずに済むが、control-ts の変更になるのでこの unit では行っていない。
2. `sweep` は呼ばれたときにだけ退役・打ち切りを行う。daemon が tick ごとに呼ぶ配線は daemon-gen の範囲。
3. 打ち切った要求の actor の中の後始末は、isolate の shutdown（actor thread の join）で行う。解決しない Promise を待つ要求は shutdown で捨てる。同期の無限ループは要求の deadline で watchdog が止めるまで join が待つ。純粋判定の deadline は 50 ms なので、この待ちは短い（実測は未確認）。

## 提案
- daemon-gen は `GenerationRegistry` の `register_deployed`・`drain`・`sweep`・`retire`・`invalidate` を DB の遷移（`script_generation_transition`）と組にし、`RetiredGeneration::cancelled_in_flight` を `script_generation_retired` に写す。
