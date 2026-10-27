---
title: operator usage の配送修復 — verify の CoS 隔離修正を取り込む
tasks: [01M5XXF7Q9FFYE8S18VMNQ34MH]
status: done
updated: 2026-10-27
---
# operator usage の配送修復

完了日: 2026-10-27

## 原因と変更

配送の prepare.log は build・全体試験・clippy・web 検査が成功した後、verify の smoke が
running のまま 61.07 秒で失敗していた。staging のログでは初回 tick が 44.485 秒、
dispatch 区間が 44.203 秒。operator usage の取り込み処理の失敗ではない。

main に既にある `phase 1: verify の CoS 背景処理を外して煙試験への干渉を防ぐ` を
`git merge --ff-only main` で取り込んだ。[採用済み ADR](../adr/2026-10-27-verify-without-cos-background.md)
に従い、verify 起動時に CoS launch を解除し、通知 cutover も抑止する。
タイムアウトと合格条件は変更しない。追加のコード変更・設計判断は不要。

元の operator usage 実装・運用手順はそのまま保持。
コミット件名: `phase 1: operator 消費の配送に verify 隔離修正を取り込み検証する`。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo build -p celeris` → exit 0。
- `cargo nextest run -p celeris-campaign --test operator_usage` → exit 0、7 passed。
  帰属4区分・未帰属・null・snapshot・CLI冪等性・競合原子性・並行取り込み・不正入力を確認。
- `cargo nextest run -p celeris --test instance_handoff -E 'test(verify_mode_never_dispatches_and_never_touches_daemon_instances)'`
  → exit 0、1 passed、7 skipped（filter）、0.634秒。隔離 DB の fake smoke 完了、CoS 表・カーソル不変。
- `docs/ops/operator-usage.md` を確認。保持設定、main/subagent の snapshot コピー、digest、対応表と再取り込みの手順あり。
- `bash scripts/dev/test-affected.sh` → exit 0。同期後の差分は文書のみで Rust 試験対象なし。
  health 検査成功（Python 38 + 1 tests、リンク・ADR・進捗参照検査成功）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --check` → exit 0。

## 未解決

実 snapshot の smoke 所要時間は次回の配送 verify で確認する。
今回、本番サービス・登録元の作業ツリーは変更せず、デプロイ・release/verify スクリプトも実行しない。

## 提案

なし。
