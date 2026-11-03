---
title: S4 robust-ts — mode ts の async pass の順序・再配送・retry・cancel・再起動・policy_version の試験
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 robust-ts — mode ts の async pass の順序・再配送・retry・cancel・再起動・policy_version の試験

完了日: 2026-11-03（unit robust-ts）。本番コードは変えていない。足したのは試験 1 file（`crates/task-dispatch/src/dispatcher/tests/cos_digest_script_ts.rs`、`cos_digest_script.rs` の子 module `ts`）だけである。

## 結論
- 親の 11 件の観点を mode `ts` で走らせる試験と、op を await している途中の出来事の試験を合わせて 15 件足した。どれも mode `ts` の pass で TS の command が Rust の検証→apply に渡る経路を通る。
- 偽の TS（`AsyncTs`）は、本物の `cosDigestPass` と同じ手順で動く。op の送り口越しに `cos_digest:list_runs` と各 run の `cos_digest:read_run` を await し、`cos_digest_decide` で command を返す。
- 読み出し口の試験専用の遅延フック（`Hook`）は偽の TS の op の前後で走る。位置は op を送る前（`Phase::Before`）と、応答を受けた後で TS の続きに渡す前（`Phase::After`。TS から見ればまだ await 中）の 2 つ。フックは次の 5 つを起こす。
  - DB の書き換え（item の判断、mode の戻し、表の改名による読み出しの失敗）
  - 遅延（`Step::Delay`）
  - generation の作り直し（`Step::GenerationGone`）
  - dispatcher の停止（`Step::Stop`。panic で tick を巻き戻す）
  - 何もしない（`Step::Go`）
- 時計: dispatcher は mode `ts` の pass を pass ごとの current-thread runtime で待つ。偽の TS は最初にその runtime で `tokio::time::pause()` を呼ぶ。deadline（200 ms）は自動で進み、実時間を待たない。外側の試験も `start_paused = true`（policy_version の試験だけは親と同じく worker の join を待つので通常の runtime）。CPU を焼く負荷と実時間の sleep は使っていない。
- どの試験でも親の `assert_invariants` で 3 つの不変条件を確かめる。
  - message は冪等 key ごとに 1 件
  - requeue は item ごとに 1 回まで（試験 DB の trigger の log で数える）
  - 記録は pass の run ごとに 1 件まで（run_id の重複が無い。`applied_by` 別より強い条件）
- Rust の基準と比べられる場面（下の表で「基準」が付いたもの）は、同じ場面を mode `rust`（PolicyScript 無し）と mode `ts` で走らせる（`ts_same_as_rust`）。message（key・text・cards・run_id）、item（source_key・state・run_id・reason）、requeue の log が byte 一致することを確かめる。

## 試験と場面
| 観点 | 試験 | 基準 | mode ts で確かめた事 |
|---|---|---|---|
| (1) 順序 | `cos_digest_script_ts_order_item_update_before_and_after_run_end` | 有 | 3 run とも `applied_by=ts`・fallback 無し・match。遅れた判断で 2 回目の digest は出ない。stopped の run だけ 1 回 requeue |
| await 中の item の更新 | `cos_digest_script_ts_item_updated_during_op_await` | 無 | `read_run` を送る前の判断は digest に載る。応答の後（TS が await 中）の判断は載らない（pass は読んだ snapshot で決める）。どちらでも requeue の `WHERE state='running'` が判断済みの item を戻さない（`applied.requeued=0`）。再配送で増えない |
| (2) 再配送 | `cos_digest_script_ts_redelivery_same_run_twice` | 有 | message・item・requeue は 1 回。digest_due の各 pass で TS は呼ばれるが、空の pass は記録を作らない |
| (3) retry: message の書き込み | `cos_digest_script_ts_retry_message_write_failure` | 有 | apply に落ちた run は記録しない。次の tick で TS の command を apply し、記録は 1 件 |
| (3) retry: 走査 | `cos_digest_script_ts_retry_scan_failure` | 有 | tick の間ずっと読めないと、tick は pass に届く前に落ちる（TS の呼び出し 0 回）。次の tick で TS の command を apply |
| op の読み出し失敗: `list_runs` | `cos_digest_script_ts_retry_list_runs_failure_during_op` | 無 | TS が `list_runs` を await している間だけ store が読めない。TS は `exception` で落ち、Rust も memo の同じ失敗で何も apply しない。同じ tick の退避が item を人へ渡すので、次の tick は message だけ書き requeue しない |
| op の読み出し失敗: `read_run` | `cos_digest_script_ts_retry_read_run_failure_during_op` | 無 | 1 つの run の `read_run` の間だけ読めない。TS も Rust もその run を飛ばす（規則 8）。読めた run だけ TS の command を apply する。次の tick で飛ばした run を apply。requeue は 1 回 |
| (3) retry: requeue の書き込み | `cos_digest_script_ts_retry_requeue_write_failure` | 有 | message は 1 件。requeue は再試行されず、item は退避のまま。記録は無い |
| (4) cancel: deadline（await 中） | `cos_digest_script_ts_cancel_deadline_during_op_await` | 有 | 1 つ目の `read_run`（run-intr）の応答の後で TS が止まる。止めた時計で deadline が打ち切り、両 run とも `applied_by=rust`・`fallback=ts_error`・`error.kind=timeout`。deadline の後は TS の op が無い（run-stop の `ops` に `read_run` が無い） |
| (4) cancel: dispatcher の停止（await 中） | `cos_digest_script_ts_cancel_dispatcher_stop_during_op_await` | 有 | TS が `read_run` の応答を待つ間に tick が panic で巻き戻る。message・requeue・記録は 0、item は running のまま。作り直した dispatcher が reconcile で pass を 1 回やり直し、run ごとに 1 回だけ TS の command を apply |
| (5) 再起動 | `cos_digest_script_ts_restart_mid_pass_applies_once` | 有 | digest 前に落ちた run は再起動後に 1 回だけ。digest 済みの run は増えない。mode の行は再起動後も `ts` |
| (5) 再起動: requeue の失敗の後 | `cos_digest_script_ts_restart_after_failed_requeue` | 有 | 再起動後も message は 1 件、requeue は 0 回、item は退避のまま |
| (7) mode と generation（await 中） | `cos_digest_script_ts_mode_and_generation_change_during_op_await` | 有 | TS が `list_runs` の応答を待つ間に mode を `rust` に戻す。apply 直前の読み直しで `mode_changed` になり、Rust の command を apply する（記録の generation は始めのもの）。`rust` の間の run は TS に渡らない。新しい generation で `ts` に戻した後の run は `applied_by=ts`・新しい generation |
| await 中の generation の作り直し | `cos_digest_script_ts_generation_gone_during_await_falls_back` | 有 | `read_run` の応答を待つ間に `generation_gone`。両 run とも `applied_by=rust`・`fallback=ts_error`・`error.kind=generation_gone`。安全違反は 0 |
| (6) policy_version と session freshness | `cos_digest_script_ts_policy_version_bump_session_fresh` | 有 | 本物の digest 経路（FakeAdapter の CoS run → worker の join → digest）。版を 7 に上げた後の run は `policy_changed` で fresh に始まり、記録される版は 7。digest は triage run ごとに 1 件、記録 2 件とも `applied_by=ts` |

ADR 付記 S4-19 の試験名との対応: 表の名前のとおり。違いは 3 つある。
- `…_ts_retry_scan_failure` を 2 つに分けた。tick 全体の失敗が `…_ts_retry_scan_failure`、op の await 中の失敗が `…_ts_retry_list_runs_failure_during_op`。
- `read_run` の失敗（`…_ts_retry_read_run_failure_during_op`）と、await 中の item の更新（`…_ts_item_updated_during_op_await`）を足した。
- `…_ts_retry_message_write_failure` などの名前は付記のまま。

## 見つけた事（挙動は変えていない）
- mode `ts` で op の await 中だけ読み出しが失敗した場合と、tick 全体で読めない場合とでは、回収のされ方が違う。前者は digest の pass だけが落ち、同じ tick の退避（CoS 無効・不在の fallback）が interrupted の run の item を人へ渡す。そのため次の tick の digest は requeue しない。後者は tick が pass に届く前に落ち、次の tick の digest が requeue する。どちらも不変条件（message 1 件・requeue 1 回まで）は守る。前者は親の「message の書き込み失敗」と同じ形である。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0「上の観点の試験が mode ts で通る」: `cargo nextest run -p task-dispatch -E 'test(/cos_digest_script/)'` → exit 0、34 passed（親の 11 件、pass-wire の 8 件、この unit の 15 件）。
- 条件 1「await 中の cancel・generation 変化の試験がある」: `…_ts_cancel_deadline_during_op_await`、`…_ts_cancel_dispatcher_stop_during_op_await`、`…_ts_mode_and_generation_change_during_op_await`、`…_ts_generation_gone_during_await_falls_back`（上の 34 passed に含む）。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。3042 tests run: 3042 passed、9 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-dispatch --tests -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲 check: 変更 path は `crates/task-dispatch/src/dispatcher/tests/` と `agent-docs/progress/` の下だけ。

## commit（件名）
- phase s4: robust-ts: mode ts の async pass の順序・再配送・retry・cancel・再起動・policy_version の試験

## 未解決事項
- 「dispatcher の停止」は、tick を panic で巻き戻すことで模した。本物の daemon の停止（process の終了）とは違う。apply は TS が返った後の 1 か所だけなので、停止の時点で DB に何も書かれていないことと、再起動後の reconcile で 1 回だけ apply されることを確かめた。process の終了と再起動の実機確認は real-host unit の範囲で、この unit では確かめていない。
- 偽の TS は Rust の参照（`cos_digest_decide`）で command を作る。本物の TS bundle を mode `ts` で走らせる試験は real-host unit の範囲で、この unit では確かめていない。

## 提案
- 無し。
