---
tasks: [01M4ZFZC0SGYE9M92CYDKST0CR]
title: MLSys 最小実装 — 固定方策の lane 規則・decision-assisted の権限・利用可否・限定エスカレーション
---
# ADR 2026-10-15: MLSys 最小実装 — 固定方策 / decision-assisted の lane 規則、利用可否、限定エスカレーション

- 日付: 2026-10-15
- 状態: 採用（純粋部は `crates/task-core/src/route_select/{lane,availability,escalation}.rs`。配線は後続 unit）
- 親 ADR: 2026-10-15-mlsys-min-decision-assisted（**D1 の優先順位は変えない**。本 ADR はその④⑤の中身を決める）
- 関連: ADR-0069（lane・天井・escalation）、2026-10-11-breaking-changes-by-default（既存の実測根拠表ルータは別機構として残す）

## Context

親 ADR D1 は「①人の明示 → ②承認・安全制約 → ③コード側の利用可否 → ④decision-assisted の分類 → ⑤fixed 方策」と決めた。
本 ADR は④⑤の規則（どの lane か）、③の判定の型、失敗後の昇格上限を、dispatcher・store・LLM から独立した純粋関数として決める。

## Decision

### D1. 優先順位（再掲・不変）
明示 model/tier・承認・安全制約（allowed_tiers・budget の `max_lane`）・利用可否が Jev に勝つ。
純粋関数は「lane の提案」だけを返し、`clamp_to_limits` で許可・天井を超える lane を返さない。
明示 lane（`TierSource::Human/System`）の task には本規則を適用しない（呼び出し側が先に分岐する。配線 unit の責務）。

### D2. 切替 `RoutingMode { Fixed, DecisionAssisted }`（既定 Fixed）
`Fixed` では分類を一切読まない。`DecisionAssisted` で分類が無い・低 confidence のときは fixed と同じ結果になり、
record の `basis` が `fixed_fallback` になる（「fixed へ戻った」と「Jev 実 API 統合が検証済み」は別の記録。後者は本 ADR の範囲外）。

### D3. fixed 方策（`fixed_lane`）
lane の候補は `Cheap / Standard / Frontier` と、作業を止めて計画を直す `Replan`。入力は構造化事実（コードが決める）。

| 作業の性質 | 結果 |
|---|---|
| 未決定の設計 | `Replan` |
| 困難な原因調査 | `Frontier` |
| 高コスト失敗につながるレビュー | `Frontier` |
| 性質が不明 | `None`（既存方策へ委ねる） |
| 通常の作業 | cheap 条件で下記 |

**cheap 条件**（4 つ全て `Yes` のときだけ `Cheap`）: ① 手順が明確 ② 機械検証可能（check 有）③ 可逆性が高い ④ 失敗影響が小さい。
いずれかが `No` なら `Standard`。`No` が無く `Unknown` が 1 つでもあれば `None`（既存方策。cheap にはしない）。通常は `Standard`。

### D4. decision-assisted の権限
jev-classifier の分類（手順確定度 `Specificity`、作業性質など）を **lane の選択入力**にする。ただし:
- cheap 条件の構造化事実（check 有・可逆性・影響）は D3 のとおりコード側で必須のまま。分類は cheap を**作れない**。
- 分類の権限は **引き上げ・提案のみ**: `NeedsImplementation` は cheap→standard、`NeedsInvestigationDesign` は frontier への提案、`InsufficientInfo` は replan への提案。`Specified` は何もしない。cheap への引き下げ権限は無い（結果の順序は常に fixed 以上。試験で固定）。
- confidence（分類の確信であって LLM の成功確率ではない。親 ADR D1）が `DEFAULT_MIN_CONFIDENCE`（0.6）未満・欠落の分類は無視する。
- task-core は jev-classifier（reqwest/tokio 依存）に依存しない。`ClassificationHint` は jev-classifier の `Specificity`・`WorkNature` と同じ語彙の狭い写しで、配線 unit（分類段）が変換する。

### D5. lane → model/source 対応表の版固定
`LanePolicyTable { policy_version, lanes }`。初期値は現行設定（`Registry::route_defaults_for(harness, lane)`）の lane 対応の写しで、
版文字列（初期値 `mlsys-min-1`）を付けて固定する。決定 record（`LaneRecord`）に `mode`・`policy_version`・`lane`・`basis` を載せる。
表を変えるときは版を上げる。実測根拠表（`routing_evidence`・`feedback`・`model_router`）には依存せず、変更もしない。

### D6. 利用可否 `Availability { Available, Unavailable(reason), Unknown }`
account/source の互換・同時実行・cooldown・quota・予算をコードで判定する。
- 不可の根拠が 1 つでもあれば `Unavailable(reason)`。
- quota・見積 cost・予算残のいずれかが `Unknown` で、不可の根拠が無いなら `Unknown`（**0 扱いしない**。「無料」「残り無限」「残り 0」のどれでもない）。
- `Unknown` は不可でも可でもない「未確認」。候補の並べ替えでは `Available` の後ろに置き、cost の順位付けでは最悪値（無限大）で扱い、予算・許可を拡張する根拠にしない。

### D7. 失敗分類と限定エスカレーション
`FailureClass { TransientInfra, Content }` と `escalation_decision(class, attempts, limits)`:
- `TransientInfra`: **同じ lane で待機・再試行**（上位 model へ送らない）。再試行上限超過は `Replan`（昇格しない）。
- `Content`: 上限付きで **1 段**だけ昇格（cheap→standard、standard→frontier）。上限（`max_escalations`、既定 1）超過、frontier での失敗、昇格先が許可・天井の外なら `Replan`。
- 返す lane は常に `clamp` 済みの許可内。

## Consequences
- 追加のみ。既存の `route_select`・`model_router`・`routing_evidence`・`feedback` は変更しない。
- 後続: wire-escalation（D7 を dispatcher へ）、wire-mode（config `routing.mode` と分類段）。
