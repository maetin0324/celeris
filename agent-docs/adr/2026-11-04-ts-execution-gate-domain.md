---
tasks: [01M6HGFR1TCHVSDDNH0GR9SNBK]
---
# ADR 2026-11-04: TS 制御層の 4 つ目の領域 `execution_gate`（Complexity Gate の得点）

- 状態: 採用（実装済み）。task 01M6HGFR1TCHVSDDNH0GR9SNBK（P2-F）。
- 関係: [境界 ADR](2026-11-02-ts-control-layer-boundary.md) の (a)・S2-3・S2-8・S5-3・S5-6・S6-4、ADR-0072 D13、ADR-0079 D4 (1)。

## 決定
1. **移す領域**: `execution_gate::decide_at` のうち規則表と強制規則（`Scorer`・`PROCESS_WORD_GROUPS`・`compound/long-and-broad`・`atomic/small`・`compound/score`・`atomic/score`）を領域 `execution_gate` として TS に同等移植する。順位の根拠は [P2-F の進捗](../progress/2026-11-03-ts-control-ops/p2f-expand-port.md)。改善は混ぜない。Rust の参照実装は消さない（S6-3）。
2. **S6-4 の改め**: S6-4 は「次の領域の選定は本番で 1 領域が `ts` に上がってから」と書いたが、人の依頼（親 task の P2-F）で「本番の compare の観測が 1 領域以上あれば着手する」に改める。本番の mode はこの領域も `rust` のままで、昇格は別の人の判断である。
3. **Rust に残すもの**: `out_of_scope_rule`、人の明示（`human/explicit`）、深さの閾値の計算（`GateThreshold`。TS は閾値を入力で受ける）、`source`・`shadow`・`depth`・`policy_version` の刻印、`direct_route::evaluate`、`update_task` と `Event::ExecutionGated` の書き込み。TS を呼ぶのは `out_of_scope_rule` に当たらず人の明示も無い task だけである。
4. **比較の場所**: `dispatcher/work_units.rs` の `execution_gate_if_needed` の 1 か所（`call_site` は `work_units.execution_gate_if_needed`）。`tree::gate::unit_gate`（計画の検証の純粋関数。store・script の口を持たない）は比べず Rust のまま。canary・`ts` でも unit の判定は Rust の規則表で決まる。
5. **入力 DTO `ExecutionGateInput`**: `features`（9 軸）・`title`・`objective`・`acceptance_total`・`artifact_exists`・`has_human`・`has_command`・`multi_environment`・`recent_budget_exhausted_ratio`・`cos_hint_compound`・`max_turns`・`tree_child`・`threshold`。工程語の数えは規則の一部なので、本文を TS に渡す。これは S2-3 の「DTO に title・objective の本文を入れない」の例外である。
6. **記録の機密除去（S2-3 の例外の条件）**: 記録する入力は `ExecutionGateRecordedInput` で、`title`・`objective` の代わりに `text_sha256`（`title\nobjective` の sha256）と `title_chars`・`objective_chars` を持つ。`input_digest` もこの値から作る。TS の例外文と不正な出力は本文を写しうるので、この領域の記録には残さない（`error.message` は固定文と大きさだけ、`ts` は `null`）。op の許可は空（scope も空）で、TS から外へ出す口は無い。
7. **出力 DTO `ExecutionGateOutput`**: `mode`・`score`・`rule_id`・`signals`（`name`・`weight`・`detail`）を宣言順の byte 列で比べる。正規化はしない。
8. **Rust の検証**: 次に落ちた出力は `invalid_output` で使わない。
   - 形・上限: 未知の欄、`signals` が 16 件を超える、`name` が 8 byte を超える、`detail` が 128 byte を超える、`name` の重複。
   - `rule_id` は上の 4 つだけ。ほかは安全違反 `gate_rule_not_allowed`（人の明示・対象外の rule_id を TS が名乗るのを含む）。
   - `mode` と `rule_id` の接頭辞の一致、`score` と `signals` の `weight` の和の一致、`compound/score` は `score >= threshold`、`atomic/score` は `score < threshold`。破れは安全違反 `gate_threshold_violated`（(a) の「深さの閾値」を Rust が当てる）。
9. **fail-closed**: TS の失敗・検証落ち・安全違反・予算切れ・呼べない generation では、Rust の参照実装の判定を書く（S5-6 と同じ `fallback`）。`title`・`objective` の合計が 256 KiB を超える task は TS を呼ばず `unavailable` を記録する。mode の遷移・権限・script-guard は他の領域と同じで、純粋判定の 1000 件の要件を使う。
10. **移行**: migration 0109 で `script_domain_modes` に `execution_gate`（mode `rust`、version 0）を足す。既存の generation の manifest はこの領域を宣言しないので、この領域の `compare` にはこの変更を含む bundle の登録が要る。

## 影響
- `decide_at` は `ExecutionGateInput` を作って `score` に渡す形に分けた。挙動と event の byte 列は変えていない（`execution_gate/tests.rs` と fixture の byte 一致で確かめる）。
- 規則表の改善は、この領域が `ts` に上がった後に TS の変更と golden の review で行う。今は Rust と TS の二重実装なので、規則を変えるときは両方を変える。
