---
title: S2 policy-split — model_policy を match_rule と decide_with に分け、lane 規則の比較 fixture を作る
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 policy-split

## 結論
- `crates/task-core/src/model_policy.rs` を挙動を変えずに 2 つに分けた。
  - `match_rule(&TaskFeatures) -> RuleMatch { rule_id, lane, condition }`: 規則表を上から当て、最初に当たった規則を返す（`lane` は天井前の proposed）。
  - `decide_with(features, ceiling, RuleMatch) -> LaneDecision`: `LaneCeiling::clamp` を当て、reasons・policy_version を付ける。
  - `ModelPolicy::decide` は `decide_with(features, ceiling, match_rule(features))` になった。`task_core` の crate 直下にも `RuleMatch`・`match_rule`・`decide_with` を再公開した。
- lane 規則の比較 fixture を `control-ts/fixtures/lane_rules/all-features.jsonl`（19,683 行）として commit した。1 行 1 件の `{"features":{…},"rule_id":…,"proposed":…,"condition":…}`。features は 9 軸の宣言順（judgment が最上位、cross_cutting が最下位）で、各軸 low < medium < high の辞書順。
- 生成は `model_policy::lane_fixture_jsonl()`（`all_features()` を `match_rule` に通す）1 つにまとめ、example `dump-lane-fixtures` と試験 `crates/task-core/tests/script_fixtures.rs` が同じ関数を呼ぶ。
- 規則ごとの件数: standard/default 13,446、frontier/judgment-under-uncertainty 3,645、frontier/costly-and-unverifiable 1,458、frontier/broad-judgment 972、cheap/mechanical-verifiable-reversible 162（合計 19,683）。
- 既存の model_policy の試験（`crates/task-core/src/model_policy/tests.rs`）は変えていない。

## commit（件名）
1. `phase s2: policy-split: model_policy を match_rule と decide_with に分ける（挙動不変）` — Rust だけの分割（model_policy.rs と lib.rs の再公開）。
2. `phase s2: policy-split: lane 規則の比較 fixture（dump-lane-fixtures と script_fixtures 試験）` — `all_features`・`lane_fixture_jsonl`、example、試験、fixture。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core -E 'test(script_fixtures) | test(model_policy)'` — exit 0、13 passed（model_policy 11 件、script_fixtures 2 件）。
- `cargo run -q -p task-core --example dump-lane-fixtures | cmp - control-ts/fixtures/lane_rules/all-features.jsonl` — exit 0（byte 一致）。`wc -l` は 19683。
- `bash scripts/dev/test-affected.sh` — exit 0。cargo 6,885 passed、21 skipped。web 697 passed、control-ts 4 passed。
- `cargo clippy --workspace -- -D warnings` — exit 0。`cargo clippy -p task-core --examples --test script_fixtures -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。

## fixture の作り直し
規則表（`RULES`）を変えたら `cargo run -q -p task-core --example dump-lane-fixtures > control-ts/fixtures/lane_rules/all-features.jsonl` で作り直す。作り直さないと `script_fixtures_lane_rules_match_committed_fixture` が落ちる。規則表の変更は `LANE_POLICY_VERSION` を上げる変更として扱う。

## 未解決事項
- なし。

## 提案
- `match_rule` が規則に当たらないときの fallback（`"no rule matched"`）は、最後の規則 `standard/default` が必ず当たるので到達しない。TS 実装（lane-rules-ts）でも同じ fallback を持つか、到達しないことを型で示すかは lane-rules-ts で決める。
