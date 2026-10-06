---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: dispatch-shadow
status: done
completed: 2026-10-05
---

# Phase 4 dispatch-shadow: task-dispatch の decision shadow

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 の task-dispatch 部分。

## 実装
- `crates/task-dispatch/src/dispatcher/routing_shadow.rs`（新規）
  - `mode = shadow` のときだけ、primary（legacy の `select_provider_for` と `select_tier`）を決めた直後、
    run の登録より前に、同じ候補行（`legacy_provider_profiles` / `enforce_round`）と同じ source 状態
    （account 帳簿・cooldown・in-use）で候補 policy（Phase 2 の enforce kernel、
    `dispatch-enforce-heuristic-v1`）の判断を純粋に計算する。順序は enforce と同じ
    （制約 allowlist → cheap はローカル行を先に → pool は残量 score 最大 → 設定順。source 状態で外れたら次）。
  - 読むだけ: health probe・HTTP・worker 起動・reviewer 追加・run 再実行をしない。`full`・`unroutable`・
    アカウント scan cache を書かない。ローカル行の health は primary がこの tick に見た結果（`ProviderSelection.candidates`
    の Down/Cooldown）だけを写す。pool の他行のアカウントは `pick_account` と同じ規則で読むだけ（`peek_account`）。
  - 記録は `Event::RoutingShadowRecorded`（kind decision・status completed）1 件を `RoutingDecided` の直後に追記。
    primary との差・除外理由・候補比較は新しい型 `DecisionShadowComparison`（version 1、
    `DecisionShadowCandidate` の列）を JSON にして `ShadowRecord.detail` に入れる（既存 pub struct に欄は足していない）。
    tokens・費用・出力 hash・予約は持たない（`ShadowRecord::validate` の decision 規則どおり）。
  - mode = legacy / enforce では計算も記録もしない。
- `dispatch_run.rs`: 上の計算と追記の 2 箇所の呼び口。`dispatcher.rs`: module 登録と型の re-export。

## 証拠
- `cargo nextest run -p task-dispatch routing_decision_shadow` → 1 passed
  - 試験 `routing_decision_shadow_never_calls_upstream_or_changes_primary`（`dispatcher/tests/routing_shadow.rs`）:
    legacy と shadow で WorkerStarted（provider/model/account）・RoutingDecided の resolution/quota_reason・
    task status・worker spawn 数（1）・local health probe 回数（1）・account 帳簿ファイルが同じ。
    event 列の差は `routing_decided` 直後の `routing_shadow_recorded` 1 件だけ。残量 20% の pool では
    legacy=p1 standard、候補=p2 frontier（差 source/model/lane、p1 は quota_low_for_lane）。同じ入力で同じ比較。
    enforce と legacy は shadow を記録しない。probe 不通時の除外理由は `unreachable`（probe し直さない）。
- `cargo nextest run -p task-dispatch` ×3 → 689 passed（一度、spawn 数を yield 待ちで数えた版が負荷下で落ちたため
  出来事待ち＋60 秒の保険に直した。直した後は 3 回とも全通過）
- `cargo nextest run -p task-dispatch -p llm-proxy -E 'test(cheap_local_first) | test(account) | test(/celeris.*(frontier|standard|cheap)/)'` → 85 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、4003 passed、12 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0（`-p task-dispatch --tests` も 0）

## 未解決事項
- decision shadow は dispatcher の worker run の経路だけ（reviewer run・planner の RoutingDecided には付けない）。
- `DecisionShadowComparison` は `detail` の JSON。task-api の audit（audit-api unit）・replay（replay unit）が読むときは
  `version` を見て解釈する。

## 提案
- audit-api / gui / web は `detail` を `DecisionShadowComparison` として解いて primary と別欄に出すと、追加の DB 欄なしで
  候補比較（除外理由つき）を表示できる。
