---
title: main-4 の評価範囲と通常運用 revision の定義
tasks: [01M5XXF7QDVASQFQBT1GZBXS6V]
status: done
updated: 2026-10-27
---
# main-4 の評価範囲と通常運用 revision の定義

完了日: 2026-10-27

## 変更

- `phase 1: main-4 の評価範囲と通常運用の支援計上を定義` — 既存契約に評価範囲だけを追記。
  新しい main-4-ops-r1 と ADR に、境界・固定条件・変更時の扱い・支援区分・主体語彙・指標の取得範囲を定義。
- 外部 operator は利用者の 2026-10-27 の決定に従って Celeris の外。結果に影響する支援は別記して試行固有に数える。
- コード・pinned bundle・pilot 実行票・過去の記録は変更しない。

## 証拠（提出時の HEAD で実行）

基準: `git merge-base HEAD main`。提出時の HEAD で実行。

- 条件 0（既存条件の保全）: `python3 /local/celeris/data/workspaces/01M5XXF7QDVASQFQBT1GZBXS6V/artifacts/check-doc-scope.py`
  → exit 0。追加した評価範囲節を除く既存契約は基準と完全一致。変更は文書 4 件のみで、pilot/pinned 条件の変更なし。
- 条件 1（新 revision と ADR）: 同コマンド → exit 0、必要節・語彙を確認。本文レビューで §1 境界、§2 固定票、
  §3 条件変更、§4 支援区分と数え方、§5 主体、§6 指標表と欠測を確認。ADR に決定と既存契約との関係を記載。
- 条件 2: `sh scripts/dev/progress-index.sh --check` → exit 0、ok。
- `bash scripts/dev/test-affected.sh --list` → exit 0、crate/web 対象なし。
- `bash scripts/dev/test-affected.sh` → exit 0。health 検査成功、Python 38 + 1 件成功、
  source-size 0 failure（既存 debt warning 3・reasoned exception 1）、docs のみのため Rust/web 試験対象なし。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `sh scripts/dev/check-doc-links.sh` → exit 0、ok。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0、302 files。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0、ok。
- `git diff --check` → exit 0。全体試験・実 LLM・release gate は実行していない。

## 未解決事項

通常運用の計測実装・採用 release の照合と実測定は別 task。本 task は文書のみ。

## 本番への移行

本番変更なし。測定開始時に別の固定票・測定 ID・承認枠を用意し、既存 bundle を上書きしない。

## 提案

なし。計測実装は既存の A5〜A8 の担当範囲を維持する。
