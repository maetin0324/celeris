---
tasks: [01M4ZFZC0SGYE9M92CYDKST0CR]
title: MLSys 最小実装 — routing.mode の配線（wire-mode）
---
# ADR 2026-10-15: `routing.mode` の配線と lane 層

- 日付: 2026-10-15
- 状態: 採用
- 親: 2026-10-15-mlsys-min-routing-policy（D1 の優先順位・D4 の権限は不変）、2026-10-15-mlsys-min-decision-assisted

## Decision

1. **config**: `[routing] mode = "fixed" | "decision_assisted"`（`crates/celeris/src/config/routing_mode.rs`）。欄・表とも任意で既定 `fixed`。未知の値・未知の欄は読み込みエラー。bootstrap と reload が dispatcher に渡す。
2. **fixed は何もしない**: lane 層は呼ばれても即 return し、lane・`celeris.route-decision/1` とも従来と同一（`lane_assist` を書かない）。
3. **decision-assisted の lane 層**（`task_core::route_select::assist_lane`、dispatcher は `dispatcher/lane_assist.rs` から呼ぶ）:
   既存方策（ADR-0069 の lane、天井込み）の結果を入力にし、`decide_lane` の提案が**既存 lane より高いときだけ**引き上げる。結果は常に既存 lane 以上
   （分類も `fixed_lane` も既存を下げない。cheap への引き下げ権限なし）。replan 提案は frontier を要求する（計画の直し自体は wire-escalation の範囲）。
   優先順位の実装: ①明示 lane（`TierSource::Human/System`）→ 提案を採らない（`overridden_by=explicit`）②許可・天井（`clamp_to_limits`）→ 外なら採らない（`ceiling`）
   ③利用可否（引き上げ先 lane で `select_route` が通らなければ採らない。`availability`）④分類 ⑤fixed。planner run には適用しない。
4. **分類は別段（次の unit classify-stage）**: dispatcher は `ClassificationBoard` を読むだけで、dispatcher・task-core・store は jev-classifier に依存しない（試験で固定）。この unit は board・型・欄まで。Jev 呼び出しの分類段は classify-stage が書く。
5. **記録**: `RouteDecision.lane_assist`（`LaneAssistRecord`）に `mode`・`classifier_status`（`jev_ok` / `fell_back_to_fixed{reason}` / `not_attempted`）・`real_api_verified`・`base_lane`・`proposed_lane`・`final_lane`・`basis`・`overridden_by`。
   `real_api_verified` は定数 `REAL_API_VERIFIED = false`。fixed に戻ったこととは別欄で、live smoke を人が承認して通すまで true にしない。
6. **credential**（board 項目の形と dispatcher 側の接続のみ。生成は classify-stage）: `KeyMissing`・`Auth` は `fell_back_to_fixed{key_missing|auth}` と、secret id・用途を添えた board 項目になる。dispatcher は未登録なら既存の `enter_credential_wait`
   （credential_gate.rs）でその unit だけを `waiting_for_auth` にする。登録済みなら待たず fixed で進む。新しい待ち機構は作らない。
7. **構造化事実**: `lane_facts(TaskFeatures)`。Medium は `Unknown`（cheap の根拠にしない）。

## Consequences
- 追加のみ。`route_select` の根拠表ルータ（`routing_evidence`・`feedback`・`model_router`）は無変更で、この経路は依存しない。
- 型の追加により `docs/api/v1/*.schema.json` と `web/api/generated/*` を再生成した。
