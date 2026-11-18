---
title: アーキv4 C 申し送り — 理由の語の読みを task-core に 1 つにする（行数の測定）
tasks: [01M7Q70NDQAK1KKXHY6WT027QZ]
status: running
updated: 2026-11-18
---
# アーキv4 C 申し送り — 理由の語の読みを task-core に 1 つにする

完了日: 未完（closeout WorkUnit の測定。task 全体の完了は統合後）

## 変更（commit 件名）
- adr: 理由の語を読む関数の置き場所と旧入口の削除を付記する
- task-core: 理由の語を読む parse_risk_reason を design_review に置き、往復の試験を足す
- dispatch-side: input.rs の理由の語の読みを parse_risk_reason に置き換え、build_health_input(_with) を消す
- dispatch-side: clippy の useless_vec と type_complexity を直す
- worker-side: review.rs の理由の読みを parse_risk_reason に置き換え、adapter.rs の試験の type_complexity を型別名で解消

## 消した対象
- `crates/task-dispatch/src/health_review/input.rs`: `build_health_input`、`build_health_input_with`、`path_pair`、覆われない class の invariant の `strip_prefix("invariant:")` の読み
- `crates/task-worker/src/change_rules/review.rs`: `reason_detail` の中の理由の語の読み
- 置き場所: `crates/task-core/src/design_review.rs` の `parse_risk_reason`（1 か所）

## 行数の測定
基準は `git merge-base HEAD main`。script と `source-size-report.toml` は基準の commit から `git show <base>:<path>` で取り出して base と HEAD の両方に当てた。
command: `python3 <script> --exclusive-classes --rev <rev> --format json`

| 指標 | base | HEAD | 差 |
|---|---|---|---|
| production 全言語 | 375763 | 375790 | +27 |
| production Rust | 331072 | 331099 | +27 |
| own_total_with_tests | 1028966 | 1029048 | +82 |

production Rust の差は +27 で、純減になっていない（受け入れ条件 3「差が 0 以下」を満たさない）。
`git diff --stat`（base..HEAD）は 11 files、+168/-75。production 側は task-core に読む関数 +21 行、
input.rs と review.rs は置き換えで減ったが、旧入口の削除を含めても task-core の追加分を上回れなかった。
試験は task-core の往復試験 +25 行と input の試験の書き換えで増えている。

## 証拠
- 受け入れ 0（読みの一本化）: `rg -n 'strip_prefix\("invariant:"\)|find\("invariant:"\)|strip_prefix\("path_class:"\)|find\("path_class:"\)' crates/` は `crates/task-core/src/design_review.rs:162,166` の 2 行（同一関数 `parse_risk_reason` 内）のみ。
- 受け入れ 1: `rg -n 'build_health_input\b|build_health_input_with' crates/` は 0 件。
- 受け入れ 2: `git diff <base> HEAD -- scripts/dev/repo-health` は空。新しい module・trait・crate は無い。
- 受け入れ 4 の cargo 検査はこの WorkUnit では未実行（closeout は測定のみ。統合後の HEAD で要再実行）。

## 未解決事項
- production Rust が +27 行で純減の条件を満たさない。review で不合格になりうる。

## 提案
- 純減が必須なら、`parse_risk_reason` の戻り値型（`RiskReason`）を小さくする、または input.rs 側の残りの変換を削る追加の task を起こす。決めるのは人。
