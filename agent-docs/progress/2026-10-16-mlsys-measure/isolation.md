---
unit: isolation
status: done
completed: 2026-10-16
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
---
# isolation: A/B/C 試行の分離

## 実装済み
- `crates/campaign/src/isolation.rs`: `prepare_trial`（試行ごとに workspace/materials/kb を初期 snapshot から新規複製、`withheld_names` の file/dir は除外、`inputs.json` に repo/materials/kb/budget の hash を記録）、`check_same_inputs`（config 間の hash 一致検査）、`finish_trial`（KB の最終 hash を返して KB を破棄）、`verify_isolation`（他試行の path・hash、全試行の final 値を含む file を報告）。
- snapshot.rs の `hash_tree` を再利用。lib.rs は `pub mod isolation;` の 1 行のみ。runner への接続は未変更（関数を呼ぶだけの接続は e2e unit の担当）。

## mock 検証済み（fixture の一時 dir）
- `cargo test -p celeris-campaign`: 全 pass（isolation 4 件: 3 config の入力 hash 一致、trial 1 の KB 書き込みが trial 2 と元 snapshot に出ない・finish で KB 破棄、final 期待値・欠陥 list・pristine テストが workspace にない、意図的な path/hash/final 値の漏れを検出）。
- `cargo clippy -p celeris-campaign --all-targets -- -D warnings`: clean、`cargo fmt --all -- --check`: clean。

## 実経路検証済み
なし。

## 未実施
- 実 case（Benchfs/Pluvio）の資産での隔離、runner の `run` からの呼び出し、workspace 全体の `test-parallel.sh`。
## 提案
- `verify_isolation` に渡す `owned_hashes` は試行固有の final hash にする（初期 hash は全 config 共通で漏れではない）。
