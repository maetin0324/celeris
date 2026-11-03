---
title: guard — (f) の停止基準で自動的に rust へ戻す監視 script_guard
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# guard — (f) の停止基準で自動的に rust へ戻す監視 script_guard

## 結論
付記 S5-7 の停止基準の監視を daemon の `crates/celeris/src/daemon/script_guard.rs` に足し、(f) の各基準について「ちょうどしきい値で戻る・手前で戻らない」を、時計を差し替えた試験で固定した。(f) の数値は 1 つも変えていない（数値は store-gen が置いた `task_core::script_guard` の定数のまま）。自動で昇格する経路は無い。dispatcher にも daemon にも LLM 呼び出しは入れていない。

## 役割の分け方
- 呼び出しごとの計数、(f) の 4 行の判定、`canary`・`ts` の `rust` への戻し（actor `system:script-guard`、CAS と同じ transaction で `script_domain_mode_changed` を追記）、安全違反での generation の `invalid` は、store-gen が置いた store の `script_guard_observe`（`script_compare_append_observed`・`script_compare_log_append_observed` から呼ぶ）が行う。計数は表 `script_guard_state` にあり、再起動の後も残る（付記 S5-7 のとおり）。dispatcher からこの関数へ呼び替えるのは canary-dispatch unit の担当。
- この unit が daemon に足したもの（`sweep`。受信箱の取り込みと同じ 30 秒の見回りで、取り込みの前に走る）:
  1. **検証の外の副作用で全体を止める**: `script_compare_log` を読み、`unvalidated_effect` が当たる記録を見つけたら `script_guard_halt` で全領域を `rust` に戻し、領域 `*` の `halted` を立てる。reason の先頭の code は `guard_unvalidated_effect`。受信箱の項目は script-inbox の経路が `script_guard_halted` の記録から作る（同じ見回りの取り込みで出る）。
     - 判定（`unvalidated_effect`）: `applied_by` が `ts` で、次のどれかに当たる記録。mode が `canary`・`ts` でない（`ts_applied_outside_ts_mode`）、安全違反がある（`ts_applied_with_safety_violation`）、TS の出力が無い・`ts_error`・`invalid_output`・`unavailable`（`ts_applied_without_valid_output`）、fallback がある（`ts_applied_with_fallback`）。純粋判定の領域は副作用を持たないので対象外。
     - 人が停止を解いた（`script_guard_cleared`）時刻より前の記録では止め直さない。process の中の cursor を再起動で失っても、この規則で同じ記録による再停止は起きない。
  2. **進む条件の判定を出すだけ**: `compare`・`canary` の領域ごとに `task_core::script_guard::readiness`（純粋判定 1000 件・CoS の経路 200 件、`canary` は TS 側の単位）を計数から求めて返し、ready になった組を log に 1 回出す。mode は書かない。昇格は人の API（api-canary）だけ。
- `tick_loop.rs` は見回り用に SQLite の store を開いて `script_guard::spawn` に渡す。開けなければ見回りを飛ばして受信箱の取り込みだけ続ける（warn を出す）。

## 試験（`crates/celeris/src/daemon/script_guard_stop_tests.rs`、時計は 2026-11-03T00:00Z からの差し替え）
| 付記 S5-12 の名前 | 確かめること |
|---|---|
| `script_guard_rolls_back_on_one_mismatch` | 一致 50 件では戻らず、不一致 1 件（窓 1000 件未満でも）で `canary → rust`・actor `system:script-guard`・受信箱 `guard_mismatch`。`compare` は mode を変えず `clean_streak` が 0 |
| `script_guard_rolls_back_on_timeout_rate_and_burst` | 0・300・601 秒の 3 件は戻らず、0・300・600 秒の 3 件で `guard_timeout_burst`。窓 502 件で timeout 2 件は戻らず、窓が 1000 件に届いた時点で `guard_timeout_rate`。1000 件の窓に 1 件（0.1%）は戻らない |
| `script_guard_rolls_back_on_failure_rate_and_streak` | 失敗 2 件と、成功を挟んだ 2 件は戻らず、連続 3 件で `guard_failure_streak`。1000 件の窓で連続しない失敗 1 件は戻らず、2 件目で `guard_failure_rate` |
| `script_guard_safety_violation_invalidates_generation` | 違反 0 件では何も変わらず、`compare` の領域の 1 件で generation が `invalid`、指す 2 領域が `rust`、受信箱に `script_generation_invalidated` 1 件と領域ごとの項目。2 回目の取り込みで増えない |
| `script_guard_unvalidated_effect_halts_all` | 判定の表。検証を通った TS の apply では止めず、検証の外の副作用 1 件で全領域を `rust` に戻して止め、受信箱に停止の項目 1 件、止まっている間は `compare` へ進めない。人が解いた後は古い記録で止め直さず（cursor を失った状態でも）、新しい記録では止める |
| `script_guard_no_automatic_promotion`（足した） | 999 件は未達・1000 件で ready、CoS の経路は 199 件未達・200 件で ready、`canary` は Rust 側の影 1000 件では未達で TS 側 1000 件で ready。どの場合も mode は変わらず、見回りは `script_domain_events` を 1 行も書かない |
| `script_guard_counts_survive_restart` | store を開き直した後も連続の失敗 2 件と 10 分の timeout の時刻 2 件が残り、3 件目の失敗で戻る |

付記 S5-12 の `script_guard_inbox_item_once_per_event` は script-inbox が `script_inbox_item_once_per_event_survives_restart` の名前で作った（同じファイルの `script_guard_tests.rs`）。付記 S5-12 は試験を `crates/celeris/tests/script_guard.rs` に置くとしていたが、`daemon` module は crate の外に公開されていないので、daemon の unit test（`crates/celeris/src/daemon/script_guard_stop_tests.rs`）に置いた。s5-close unit が付記と突き合わせるときはこの対応を使う。

## 証拠（提出時の HEAD で実行、基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris -E 'test(script_guard)'`: exit 0、11 件すべて合格（この unit の 7 件と script-inbox の 4 件）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象 crate は `celeris`・`celerisctl`。908 件合格・0 skipped、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## commit
- phase s5: guard: 停止基準の見回り（検証の外の副作用で止める・進む条件を出すだけ）と (f) のしきい値の試験

## 未解決
1. `sweep` は `script_compare_log` を毎回全件読み、process の中の cursor で新しい行だけを判定する。表が大きくなると読みが重くなる。id で絞って読む store の関数（task-core）を足すのは、task-core を変える後の unit に任せた（この unit は task-core に触れず、test-affected を celeris に留めた）。
2. 検証の外の副作用の検出は `cos_digest` の記録の自己矛盾（`applied_by` と mode・出力・安全違反の組）で行う。apply が記録を書かずに副作用を起こした場合は検出できない。apply を記録と同じ transaction に置く設計（S4-14・S5-6）に頼っている。
3. 実 daemon を通した戻しと受信箱の確認は gen-e2e unit に任せる。

## 提案
- canary-dispatch が `script_effect_unverified` で領域を戻すとき、`applied_by: ts` の記録の不整合を見つけたら、この unit の `unvalidated_effect` と同じ規則で `script_guard_halt` を呼ぶと、30 秒の見回りを待たずに止められる（統合の段で検討）。
