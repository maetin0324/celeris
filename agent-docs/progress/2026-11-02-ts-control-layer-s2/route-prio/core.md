---
title: S2 route-prio core — route_priority を列と basis に分け、出力 DTO と fixture 生成を作る
tasks: [01M6EJYCFFSJXYBVH87Q8FSNYB]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 route-prio core

完了日: 2026-11-02

## 結論
- `llm_route::route_priority_decide(registry, harness, lane, cos, execution_class) -> (RoutePriorityBasis, &[CandidateKey])` を作り、`route_priority` は同じ signature の薄い包みにした（挙動不変）。basis は `cos`・`execution_class`・`lane`・`harness`・`none`。選んだ列が空なら列の種類によらず `none`。空の列から下の列へ落ちない順序と、lane が `None` のときの `unwrap_or_default` は変えていない。lane 列→harness 列の引き方は `Registry::route_defaults_for` と同じ private 関数 `route_defaults_column` を共有する。
- 規則を境界 ADR に付記 S2-10 として足した。
- `task_core::script_compare` に `RoutePriorityOutput { basis, candidates }`（`deny_unknown_fields`、宣言順）を足し、basis の enum（snake_case。正本は `llm_route::RoutePriorityBasis`）を `pub use` で出した。
- `task_core::script_compare::route_priority_fixture_jsonl()`（`script_compare/route_priority_fixture.rs`）が手書き例 26 件と生成例 2000 件の JSONL を作る。生成は手書きの SplitMix64（固定 seed）で、4 harness の合成 `Registry` の候補列だけを埋め、`RoutePriorityInput::redacted` に通した入力と `route_priority_decide` の出力を書く。入力が同じ例は作らない。新しい依存は足していない。
- example `dump-route-priority-fixtures` が stdout に出し、`control-ts/fixtures/route_priority/cases.jsonl`（2026 行、約 1.1 MB）を commit した。basis の内訳は生成例で none 805・harness 554・lane 327・cos 202・execution_class 112。
- `crates/task-core/tests/script_fixtures.rs` に 6 件の試験を足した（既存の `llm_route/tests.rs` は不変）: commit 済み file と生成の byte 一致と行数の下限、手書き 16 件以上・生成 2000 件・5 種の basis が手書きと生成の両方に現れること、重複の行・case 名・入力が無いこと、入力から registry を組み直して `route_priority_decide` と `route_priority` が同じ出力を返すこと、手書き例の basis と候補数を別に固定、出力 DTO の直列化の形。
- 手書き例が含むもの: class あり／未知の class／空の class 列／別の harness だけの class、lane 列あり／未知の lane／大小文字違いの lane／空の lane 列／lane が空文字／lane が null と空文字の行の組、行の無い harness、空の harness 列、lane 行だけで harness 列が無い、cos で候補あり／空／class 付き、候補の重複、非 ASCII の id、JSON の escape が要る id、`__proto__`・`constructor`・`toString` の lane・class 名、表が全部空（cos あり・なし）。

## commit（件名）
1. `phase s2: route-prio core: route_priority を列と basis を返す route_priority_decide と薄い包みに分ける（挙動不変）・付記 S2-10`
2. `phase s2: route-prio core: RoutePriorityOutput・basis の enum と route_priority の fixture（example dump-route-priority-fixtures・cases.jsonl・試験）`
3. `phase s2: route-prio core: 進捗の記録`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo run -q -p task-core --example dump-route-priority-fixtures | cmp - control-ts/fixtures/route_priority/cases.jsonl` → 差分なし。
- `cargo test -q -p task-core --test script_fixtures` → 8 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6953 passed・21 skipped。対象は task-core と逆依存 crate、web と control-ts の typecheck+test）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は ADR・本記録・cases.jsonl・task-core の 5 file だけ。

## 提案
- lane が `None` のとき `unwrap_or_default` で空文字 `""` の lane 行を引く（手書き例 `hand/lane-null-takes-empty-string-row`）。config が `""` の lane を許すなら、lane の無い run が意図せずその行を使う。直していない。config の読み込みで空文字の lane を拒むか、`None` を「lane 行を引かない」にするかを決めるとよい。
- 空の宣言の列（class・lane・cos）は下の列へ落ちず、`NoDeclaredDefault` で fail-closed になる。harness 全体の `route_defaults` が空配列のときと未宣言のときは区別できない（どちらも basis `none`）。現行の意図どおりと読んだが、空の class 列を config で書けると誤設定で run が止まる。config の読み込みで空の列を拒むかを検討してよい。

## 未解決
- なし（TS 実装・結線・本物の host の試験は後続の unit）。
