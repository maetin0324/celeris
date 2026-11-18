---
title: アーキv4 C 申し送り — 理由の語の読みを task-core に 1 つにする（行数の測定）
tasks: [01M7Q70NDQAK1KKXHY6WT027QZ]
status: done
updated: 2026-11-18
---
# アーキv4 C 申し送り — 理由の語の読みを task-core に 1 つにする

完了日: 2026-11-18

## 変更（commit 件名）
- adr: 理由の語を読む関数の置き場所と旧入口の削除を付記する
- task-core: 理由の語を読む parse_risk_reason を design_review に置き、往復の試験を足す
- dispatch-side: input.rs の理由の語の読みを parse_risk_reason に置き換え、build_health_input(_with) を消す
- dispatch-side: clippy の useless_vec と type_complexity を直す
- trim-lines: parse_risk_reason の戻り値を組にして enum と重複を削る
- trim-lines: covered_pairs を flat_map にし、読みの周辺を整理する
- worker-side: review.rs の理由の読みを parse_risk_reason に置き換え、adapter.rs の試験の type_complexity を型別名で解消

## 消した対象
- `crates/task-dispatch/src/health_review/input.rs`: `build_health_input`、`build_health_input_with`、`path_pair`、覆われない class の invariant の `strip_prefix("invariant:")` の読み
- `crates/task-worker/src/change_rules/review.rs`: `reason_detail` の中の理由の語の読み
- 置き場所: `crates/task-core/src/design_review.rs` の `parse_risk_reason`（1 か所）

## 行数の測定
基準は `git merge-base HEAD main`。script と `source-size-report.toml` は基準の commit から `git show <base>:<path>` で取り出して base と HEAD の両方に当てた（`--config` で指定）。
command: `python3 <script> --exclusive-classes --rev <rev> --format json`（production は views.product_only の lines と rust_lines、own_total は views.own_total_with_tests）。測定は trim 統合後の HEAD で再実行し、表の値は同じだった。

| 指標 | base | HEAD | 差 |
|---|---|---|---|
| production 全言語 | 375763 | 375760 | -3 |
| production Rust | 331072 | 331069 | -3 |
| own_total_with_tests | 1028966 | 1029010 | +44 |

production Rust は -3 で純減。trim-lines の変更: `RiskReason` enum を消し `parse_risk_reason` の戻り値を `(kind, class, path か名前)` の組にした。input.rs の `own_reason` を 1 行に畳み、`path_class_pair` の doc と `covered_pairs` の rustfmt で増えた行を `flat_map` で整理した。adapter.rs の試験の型別名 `EnvLog` は base に戻した（`cargo clippy --workspace -- -D warnings` は通る。`--all-targets` の type_complexity は base からある既存の指摘で、この task の範囲外）。
own_total_with_tests は往復の試験・input の試験の書き換えで +44（試験の契約は弱めていない。往復の試験と覆われない class の invariant の試験は残した）。

## 証拠
- 受け入れ 0（読みの一本化）: `rg -n 'strip_prefix\("invariant:"\)|find\("invariant:"\)|strip_prefix\("path_class:"\)|find\("path_class:"\)' crates/` は 0 件。読みは `crates/task-core/src/design_review.rs` の `parse_risk_reason`（`split_once("path_class:"|"invariant:")`）の 1 か所だけで、input.rs の `own_reason` と review.rs の `reason_detail` はそれを呼ぶ。
- 受け入れ 1: `rg -n 'build_health_input\b|build_health_input_with' crates/` は 0 件。
- 受け入れ 2: `git diff <base> HEAD -- scripts/dev/repo-health` は空。新しい module・trait・crate は無い。ADR 付記は実装前の commit。
- 受け入れ 3: 上の表。production Rust -3。
- 受け入れ 4（trim 統合後の HEAD で再実行、recheck-final。提出時の HEAD で実行）: `bash scripts/dev/test-affected.sh` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`cargo nextest run -p task-core -p task-dispatch -p task-worker -E 'test(/design_review|health_review|review_risk_|boundary_case_|change_rules/)'` exit 0（197 件通過、4399 件は対象外）。

- recheck-final の実行結果: (a) nextest 197 件実行・197 通過・4399 件対象外・exit 0、(b) `bash scripts/dev/test-affected.sh` exit 0（最後に「試験対象の変更なし」と出た。repo_health_gate は all checks passed）、(c) `cargo clippy --workspace -- -D warnings` exit 0、(d) `cargo fmt --all -- --check` exit 0、(e) 測定は上の表のとおり production Rust 331072→331069（-3）。全体の試験（nextest --workspace）は走らせていない。

## 未解決事項
- 無し。

## 提案
- 無し。
