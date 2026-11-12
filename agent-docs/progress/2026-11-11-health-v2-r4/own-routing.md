---
title: "健全性v2 R4 own-routing: routing の状態を RoutingRuntime へ移し route_state の判断を純粋関数に分ける"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: running
updated: 2026-11-12
---
# 健全性v2 R4 own-routing: routing の状態を RoutingRuntime へ移し route_state の判断を純粋関数に分ける

完了日: 未完了（2026-11-12 時点。提出時の HEAD の検査の後に書く）

## 概要

- `Dispatcher` の 15 field（`llm_route_registry`・`harness_adapters`・`account_limits`・`accounts_scan_cache`・`login_pending_accounts`・
  `account_books`・`source_cooldowns`・`routing_mode`・`classification_board`・`classification_wait`・`classification_deferred_since`・
  `warned_unroutable`・`unroutable`・`dispatch_routing`・`routing_context_registry`）を `crates/task-dispatch/src/routing_runtime.rs` の
  `RoutingRuntime` 1 個（`Dispatcher::routing`）に移した。field は private。`Dispatcher` の側は method を呼ぶ入口だけ。
- `route_state`（約 220 行）の判断は `routing_runtime/route_state.rs` の純粋関数 `build_route_state(&RouteStateInputs) -> RouteState`。
  走っている run・review の `(source, account)` は `Dispatcher::route_state` が集める。走査 cache の更新と帳簿の lock は
  `RoutingRuntime::route_state` が行う。`Dispatcher::route_state` は入力を組んで owner を呼ぶだけ（約 30 行）。
- `RouteState`・`account_adapter_for_source_kind`・`source_account_pool`・`DispatchRoutingSettings`・`ClassificationBoard` 系の型は
  `routing_runtime` に移した（owner が `dispatcher` module を使わないため）。`task_dispatch::` 直下の公開 path は変わらない。
- 基準は `$CELERIS_WU_BASE`。quota（`quota_activity`・`quota_calibration`）は own-quota、`eval_runs`・`knowledge_probe_cache` は対象外で `Dispatcher` に残した。

## 実装前 brief

1. 根本原因: routing の状態 15 field が `Dispatcher` の field で、`dispatcher/` のどの module からも書ける。`route_state` は判断（account 一覧・
   cooldown・上限から admission の状態を組む）と cache の更新が 1 関数に混ざっていた。
2. 責務 owner: `crate::routing_runtime::RoutingRuntime`（状態）と `routing_runtime::route_state::build_route_state`（判断）。
3. 既存の再利用先: `evaluate`・`source_account_pool`・`effective_account_limit`・既存の `route_select`・`lane_assist`・`cos_slot_wait` ほかの試験。
4. 選択肢: (a) field を struct に包むだけ（ADR で不採用）。(b) 処理ごと owner に移し、判断は純粋関数に分ける。b を採った。
5. 新しい状態・分岐・fallback・port: 無し。`RouteStateInputs`・`RouteStateContext` は借用の束。
6. 撤去対象: `Dispatcher` の 15 field と初期化、`route_state` の旧い本体、`route_select.rs` の `RouteState` と 2 関数の定義、`lane_assist.rs` の
   board の定義、`dispatch_run.rs` の `DispatchRoutingSettings` の定義。試験の field 直参照。module map に移した module の旧い行は無かった。
7. 不変条件: 費用不明で止める・予算の予約と解放の順序・`token_budget.rs`・`credential_gate.rs` の判定は変えない（この unit はその file を触らない）。
   `route_state` の出力は旧い実装と同じ（既存の `route_select`・`class_route`・`cos_slot_wait`・`cos_capacity` の試験を同じ assertion のまま通す）。

## 試験の変更

- 足した試験 `structure_guard_routing_runtime`（`routing_runtime.rs`）: 不変条件は ADR D2 の 1〜3・7（field が private、`#[cfg(test)]` の field が無い、
  15 件、`Dispatcher` を受けない）と、純粋 module が `Dispatcher`・store・時計・lock の語を持たないこと。既存の試験は振る舞いだけを見るので、field を `pub` に戻しても通る。
- 足した試験 4 件（`route_state.rs`）: CoS の予約は CoS の判断で宣言済み source だけ数える、暗黙 account の cooldown は期限まで、
  上限は保存値を読めたときだけ書く（CoS の +1 は暗黙 account に付けない）、OAuth プールはログイン済みだけ出す。route_state の収集と起動の間を挟まない既存の試験は
  構築済みの `RouteState` を渡すので、組み立ての条件は検出できなかった。
- 既存の試験は assertion を変えていない。`d.source_cooldowns`・`d.account_limits` の直参照を `#[cfg(test)]` の method に置き換えた。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/structure_guard|route_state|routing_runtime|class_route|route_select|lane_assist|cos_slot_wait|cos_launch_cross|cos_capacity|account|route_launch|jev|snapshot|source_account/)'`: 186 passed（初回の提出時）。
- `bash scripts/dev/test-affected.sh`: exit 0（celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、3747 passed、564 秒、load 15〜27）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: ok。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head HEAD --policy-version 1`: exit 0（verdict=pass、worsened=0）。

## 初回の提出の check の不合格と直し

- `celeris:repo-health/1`（基準は WU の base）: `Dispatcher` の method の数が base の 394 から 395 に増え（`route_select.rs` に足した
  `route_holds`）、`size.responsibility-growth` が worsened になった。手元の `check-health.sh` は基準が `git merge-base HEAD main` で、
  own-cluster・own-maint の減少を含めて比べるので通っていた。`route_holds` を `Dispatcher::route_state` の本体に畳み、394 に戻した。
- `bash scripts/dev/test-affected.sh`: exit 100。`celeris-campaign::dagbench_process recovery_notice_continuation` が 600 秒で TIMEOUT。
  単独では 5 回とも pass（30〜45 秒）、上の再実行でも pass。`RoutingRuntime::route_state` は帳簿をまとめて lock するが、他の lock の持ち手
  （llm-proxy・routing_guardrail・sinks）は帳簿を 1 つずつしか持たないので、lock の順序による待ち合いは起きない。負荷による時間切れと見た（原因は未確認）。

## 2 回目の提出の check の不合格と、fix-cmp-home による直し

- 計画の check `bash scripts/dev/test-affected.sh` が exit 100 だった。落ちたのは `celeris::script_compare_cos_digest` の 6 件だけ
  （3747 件中 3741 件 pass）。`digest_pass` の 2 回目の tick の後の `assert_eq!(world(&env), …)` で、受信箱に
  `release_unpromoted-selfdeploy`（fallback）の項目が増えていた。
- 原因は routing の変更ではなかった。`crates/celeris/tests/support/script_compare.rs` の `config_text` が `[selfdeploy] releases_dir` を
  書かないので、試験の dispatcher が既定の `~/.local/celeris/releases`（本物の host）を読み、host の検証済みで未昇格の release が
  `release_unpromoted_secs`（既定 3600 秒）を過ぎると停滞の判定が項目を作っていた。本物の HOME で 6 件 FAIL、
  `HOME=$TMPDIR/empty-home` で 9 件すべて pass を確かめた。
- この file は own-routing の範囲の外なので、この unit では直さず計画の直しを申告した。計画 v4 が WU fix-cmp-home
  （`config_text` に `[selfdeploy]` と `releases_dir = "releases"` を足し、config が一時 dir の外を読まないことを assert する）を足し、
  own-routing の依存先を fix-cmp-home に変えた。celeris が旧い WU branch を `own-routing-superseded-*` に退避し、fix-cmp-home の HEAD から
  作り直した。この run は退避した branch の『own-maint の test-affected の結果を書く』より後の 5 commit を古い順に cherry-pick した
  （衝突なし、実装は作り直していない）。作業ツリーに fix-cmp-home の直しがあることは `config_text` の `[selfdeploy]` で確かめた。

## 未確認・提案

- guard を外した確認（field を `pub` にして `structure_guard_routing_runtime` が落ちること）は未実施。
- `CredentialGate` は credential の path の型なので crate 直下の re-export で参照している。credential の owner を作る unit が型の置き場を決める。
- `test_cos_reserved_sources`（試験用 hook）は `Dispatcher::route_state` の `#[cfg(test)]` の後処理に残した。own-hooks が移す。
