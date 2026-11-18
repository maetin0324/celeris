---
tasks: [01M7NZXF1WCP8ADXKK1ENGS1PW]
---
# ADR 2026-11-18: PathClass に任意の欄 invariants を先に足す

- 日付: 2026-11-18
- 状態: Accepted
- 関係: ADR 2026-11-17-boundary-change-risk-review（アーキ v4 C。この ADR はその D2・D6 の一部を先に入れる）、
  [ADR 2026-11-07 health review integration](2026-11-07-health-review-integration.md)（`PathClass` と `ReviewPolicy`）。
- task: 01M7NZXF1WCP8ADXKK1ENGS1PW（アーキ v4 C 前提。unit invariants）

## 決定
- D1: `task_core::design_review::PathClass` に `invariants: Vec<String>` を足す。
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]` を付け、書かない JSON は今までどおり読め、空なら書き出さない。
  値は `ReviewPolicy.invariants` の key（不変条件の名前）を指す。
- D2（C の D2・D6 の一部）: 欄の追加と、`PathClass {` の全 literal（task-core・task-dispatch・task-worker の試験）の更新、
  生成 schema（`docs/protocol/worker-protocol.schema.json`）の再生成、往復の 2 試験だけを先に入れる。
- D3: この変更では `assess_risk`・`high_risk_classes`・reason・prompt を変えない。`invariants` は読み書きされるだけで、
  risk の判定には使わない。使う規則・reason・prompt の変更は C が行う。
- D4: `scripts/dev/repo-health/` と profile の `policy_paths` は変えない。
- D5: C の branch を同期して衝突したら、risk の規則に関わる側（C 側）を残す。この ADR の範囲は D1〜D4 だけである。
- ADR 番号は採らない（日付+slug、ADR-0128 D5）。

## 理由
欄の追加（型と schema の変更）と、それを使う規則の変更を分けると、後者の差分が規則だけになり review しやすい。
また、`invariants` が読み捨てられる不具合は往復の試験でしか検出できない。

## 撤去対象
無し（足すだけで、置き換える旧い仕組みは無い）。
