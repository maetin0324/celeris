---
title: S2 route-prio hook — task の run の route 選択に route_priority の compare を結線する
tasks: [01M6EJYCFFSJXYBVH87Q8FSNYB]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 route-prio hook

完了日: 2026-11-02

## 結論
- `task_dispatch::script_policy::PolicyScript` に `route_priority(RoutePriorityInput, deadline)` を足した。daemon の `ScriptPolicy`（`crates/celeris/src/daemon/script_policy.rs`）は両領域を同じ `call_domain`（scope 空、S2-7）で actor に送る。
- `classify_route_priority` を足した。`basis` と `candidates` の byte 一致で `match`／`mismatch`、候補 256 件超・未知の basis・未知の欄は `invalid_output`、TS の候補が入力 `table` のどの列にも無ければ `safety_violations` に `undeclared_candidate`（S2-8）。失敗（deadline 超え・`PolicyScriptError`）の分類は両領域で共有する。
- `CompareDraft` は領域（`DraftDomain::LaneRules`／`DraftDomain::RoutePriority`）と `run_id` を持つ形に直した。`call_site`・`domain` は領域から決まる。`call_lane_rules` に加えて `call_route_priority` を公開した（real-host unit 用）。
- `dispatcher/script_compare.rs` は tick ごとに両領域の mode 行を一度に読み、generation の確認・`unavailable`・deadline・in-flight 上限・記録を `compare_domain` で共有する。mode が `rust` なら DTO を作らない。
- 結線は `dispatch_run.rs` の `select_route_with` の直後の 1 行だけ（`route_select.rs` は不変）。`cos` の判定・review_spawn・lane_assist の試走・`order_by_headroom`・frontier 列の後ろの列では呼ばない。比べる harness は `RouteTrace.harness` で、1 判定 1 件。決定を境界 ADR の付記 S2-11 に書いた。

## commit（件名）
1. `phase s2: route-prio hook: PolicyScript に route_priority・classify_route_priority・領域を持つ CompareDraft・dispatch_run の select_route_with の直後の compare・試験・付記 S2-11`
2. `phase s2: route-prio hook: 進捗の記録`

## 試験
- `crates/task-dispatch/src/dispatcher/tests/script_compare_route.rs`（新規 10 件）: match（1 判定 1 件・call_site・run_id・入力の harness/cos/class）、mismatch（宣言済みの別の列と `undeclared_candidate` の 2 通り）、例外と不正出力（257 件・未知の basis・null）、timeout（`start_paused`）、mode rust で呼ばない、bundle 違いと PolicyScript 無しの `unavailable`、failover（`claude_code` が `NoDeclaredDefault` で断り `test` が選ぶ）で 1 件、空の列で basis `none` のまま `NoDeclaredDefault`、lane_assist の試走で呼ばない、cos で呼ばない、review_spawn で呼ばない（worker の run の 1 件だけ）。
  route の比較は `harness_selected`・`llm_route_selected`・`routing_decided` の event の直列化、run の行（role・seq・adapter・model・account・work unit）、route refused の progress を、run id（ULID）だけ伏せて mode rust・PolicyScript 無しの基準と文字列一致で比べる（task id は固定）。
- `crates/task-dispatch/src/script_policy_tests.rs` に 3 件: `classify_route_priority` の match／mismatch／`undeclared_candidate`、`invalid_output` と失敗の分類、下書きの記録（run_id・call_site・他の harness の列を写さないこと）。
- 既存の `tests/script_compare.rs`（lane_rules）は偽の script を両領域に広げただけで、試験の中身は不変。`review_route.rs` は helper 3 つを `pub(super)` にしただけ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(script_compare) | test(review_route) | test(lane_assist)'` → 51 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 2981 passed・9 skipped。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `python3 scripts/dev/check-source-size.py` → 0 failure(s)。`dispatch_run.rs` は +2 行（1708→1710）。

## 未解決事項
- なし（TS 実装は ts unit、本物の host の試験は real-host unit）。

## 提案
- compare が比べるのは `route_select_one` の最初の `route_priority` の列で、frontier lane のときに frontier 列を前に足した後の `RouteTrace.candidates` ではない（S2-1 の範囲どおり）。frontier 列の前置きを TS に移すなら別の領域として DTO を決める。
- 現行の挙動の観察（コードを読んで確かめた。bug かは未確認で、直していない）: `route_select` が全 harness で断ったとき、`RouteTrace.harness` は最後に試した harness になり、`RouteTrace.candidates` は設定されず空のまま残る（成功の分岐だけが設定する）。
