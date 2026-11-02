---
title: S1 runtime-guard — watchdog による V8 中断・cancel・generation の破棄・heap/input/in-flight 上限・監視
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
updated: 2026-11-02
---
# S1 runtime-guard — ScriptRuntime の安全装置

完了日: 2026-11-02

## 結論: terminate した isolate は再利用せず、作り直す
- watchdog が V8 を止めた generation と、heap 上限で止めた generation は捨て、同じ script で新しい isolate を作る（generation 番号 +1）。
- 試験で確かめたこと:
  - 再利用は技術的にはできる。`cancel_terminate_execution` の後なら同じ isolate で次の script が動く
    （unit 試験 `actor::tests::terminate_then_reuse_keeps_half_written_state`）。
  - ただし止めた時点の途中の状態が残る。同じ試験で `globalThis.pairA = 1; for (;;) {} globalThis.pairB = 1;` を止めると、
    再利用した isolate では `[pairA, pairB]` が `[1, null]` になる。対で書く global の片方だけが書かれた状態で次の要求に進んでしまう。
  - 作り直すと global は初期状態に戻り、次の要求が通る（`runtime_timeout_sync_loop`: 止めた後の `add` は generation 1 で 42、
    止める前に書いた `mark` は null）。
- 理由:
  1. script の不変条件（global・module の状態）は V8 の中断で途中のまま残り、Rust の側からは直せない。判定を返す層が半端な状態で
     動き続けるより、捨てて初期状態から評価し直す方が安全。
  2. 中断した generation の他の未完の要求（Promise・async op）は中断で microtask が捨てられ、結果が来るか分からない。
     generation ごと捨てると、それらに `GenerationGone` を確実に返せる。
  3. 作り直しの費用は script の評価 1 回で、watchdog・heap 上限の発火は異常時だけ。
  4. heap 上限で止めた isolate は既に上限の近くまで heap を使っており、再利用しても次の要求ですぐ上限に当たる。

## 変更
- `phase 1: runtime-guard: watchdog による V8 中断・cancel・generation の破棄・上限・監視`
  - `crates/script-runtime/src/watchdog.rs`（新規）: 専用 thread 1 本。actor は V8 に入る直前に deadline を `arm`、戻ったら `disarm`。
    deadline を過ぎても `disarm` されなければ、その generation の `IsolateHandle::terminate_execution` を呼ぶ。時間は呼び出し側の deadline だけで決まる。
  - `src/actor.rs`: V8 に入る 2 か所を watchdog で守る。
    - 入口の呼び出し（同期の部分がここで走る）: その要求の deadline。止まったらその要求に `Timeout`、他の未完に `GenerationGone` を返し作り直す。
    - `poll_event_loop`: 未完の要求の最も早い deadline。入る前に deadline を過ぎた要求は `Timeout` で外す（その要求のために他の要求の実行を止めない）。
      止まったら最も早い deadline の要求に `Timeout`、残りに `GenerationGone`。
    - 作り直しは古い generation（未完の future → 入口の関数 → `JsRuntime` の順）を落としてから新しい isolate を作る
      （V8 の isolate は作成と逆順に落とす必要がある）。
    - heap 上限: `CreateParams::heap_limits(0, 64 MiB)` と `add_near_heap_limit_callback`。上限に近づいたら `terminate_execution` し、
      畳むための猶予として上限を 2 倍にする。猶予は generation ごとに 1 回で、2 回目は広げない（V8 が process を落とす）。
      止まった要求に `HeapLimit`（どの要求か分からない event loop の中なら未完の全部に `HeapLimit`）を返し作り直す。
    - restart: handle からの制御 channel（bounded queue とは別。満杯でも届く）。未完の要求に `GenerationGone` を返し作り直す。queue に残る要求は新しい generation で動く。
    - cancel: 受け手（oneshot）が落ちた要求は結果を捨て `discarded` を数える（skeleton の挙動に数を足した）。
  - `src/handle.rs`: `RuntimeLimits { input_limit_bytes: 1 MiB, heap_limit_bytes: 64 MiB }`（`start_with_limits`、`start` は既定値）。
    入力（domain の byte 数と input の JSON の byte 数の和）が上限を超えたら actor に送らず `InputTooLarge`。
    in-flight の上限は既存の `MAX_IN_FLIGHT` 64。`restart()`・`ScriptReply.generation` を足した。
    `RuntimeStats` に generation・queued・discarded・watchdog_terminations・heap_limit_hits・restarts・busy_rejections・timeouts・failures を足し、
    `stats()` が actor 側と handle 側の数を合わせて返す。
  - `src/error.rs`: `InputTooLarge { size, limit }`・`HeapLimit` を足した。
  - `tests/runtime.rs`: 下の 8 試験を足した。
- deno_core の型は crate の外に出していない（公開は handle・DTO・`RuntimeLimits`・`ScriptError`・定数だけ）。

## 試験（`crates/script-runtime/tests/runtime.rs`）
| 試験 | 確かめること |
|---|---|
| runtime_cancel | 呼び出し側の future を落とすと actor が結果を捨てる（`in_flight` 0・`discarded` 1）。後で Promise を解いても runtime は使える |
| runtime_timeout_sync_loop | 同期の無限ループを deadline 200 ms で watchdog が止め（`Timeout`、`watchdog_terminations` 1）、その後の要求が generation 1 で通る |
| runtime_restart | restart で generation が 0→1→2、global は初期化。shutdown 後の restart は `Terminated` |
| runtime_generation_discard | Promise を待つ 2 要求が restart で `GenerationGone`。新しい generation の gate は新しい要求だけを解く |
| runtime_heap_limit | 小さな割り当ての繰り返しが 64 MiB で `HeapLimit`、process は落ちず generation 1 で次の要求が通る |
| runtime_input_limit | ちょうど 1 MiB は通り、1 byte 超えは `InputTooLarge { size: 1048577, limit: 1048576 }`（actor に届かない） |
| runtime_inflight_limit | 64 本で in-flight が満ちると次の要求は queue に留まる。1 本 cancel すると動き出し、残り 63 本も解ける |
| runtime_oom_child_process | 試験 binary 自身を子 process（env `CELERIS_SCRIPT_OOM_CHILD=1`）で起こし、256 MiB の文字列を作らせる。親は子が signal で落ちた（panic の exit 101 ではない）ことと、子が `OOM_CHILD_SURVIVED` を出していないことだけを見る |

- 時間は注入した deadline（`ScriptCall.timeout`）と出来事待ち（`watch_stats().wait_for`）で決める。CPU を焼くのは `runtime_timeout_sync_loop` の
  ループ 1 本（200 ms）と unit 試験のループ 1 本（100 ms）だけ。
- 子 process を手で起こした結果: exit 133（SIGTRAP）、stderr に `Fatal JavaScript out of memory: Reached heap limit`。
  GC の log では heap が 256.9 MB に達してから落ちている。1 回の大きな割り当て（large object）は上限と猶予を超えて確保された後に
  GC が回収できず落ちる、と読める（V8 の内部は未確認）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-script` → exit 0、13 tests run: 13 passed（結合 12 = skeleton の 4 + この unit の 8、unit 1）。3 回続けて同じ結果
- `CELERIS_SCRIPT_OOM_CHILD=1 <runtime の試験 binary> --exact runtime_oom_child_process --nocapture` → exit 133（SIGTRAP）
- `bash scripts/dev/test-affected.sh` → 対象 `celeris-script`、exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo clippy -p celeris-script --tests -- -D warnings` → exit 0（写しの deno_core の deprecated 警告 1 件だけ。error ではない）
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- 起動時と作り直し時の script の評価（`execute_script`）は watchdog で守っていない。評価に deadline を与える欄が無いため。
  同じ script は一度評価できているので作り直しでは止まらない想定だが、bundle-load で load の deadline を決めるなら足す。
- event loop の中で heap 上限に当たったときは、どの要求のせいか分からないので未完の全部に `HeapLimit` を返す。
- native async op はまだ無い（host-api の範囲）。generation を落とすと `JsRuntime` と一緒に op の future も落ちる設計で、op を足したら
  `runtime_generation_discard` と同じ形の試験を op 版で足す。

## 提案
- 無し。
