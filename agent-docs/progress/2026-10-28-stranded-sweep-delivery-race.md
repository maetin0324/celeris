---
title: 配送と sweep の競合による古い未統合通知を防ぐ
tasks: [01M60JC1N9V08GHGDSEHP27TRC]
status: done
updated: 2026-10-28
---
# 配送と sweep の競合による古い未統合通知を防ぐ

完了日: 2026-10-28

## 変更

commit: `phase 1: 配送と競合した sweep の古い未統合通知を防ぐ`

- sweep は祖先判定前の delivery と integration の snapshot に束縛して保存する。
- SQLite の IMMEDIATE transaction 内で一致を検査し、配送中の unintegrated は保存しない。
- integrated への更新は配送中も許し、既存の受信箱項目を自動で解消する。
- 人の hold / discard と他の統合結果を古い判定で上書きしない。
- [ADR](../adr/2026-10-28-stranded-sweep-delivery-race.md) に判断を記録した。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo test -p task-ops --lib stranded::sweep_race_tests` → exit 0、6 passed。
  hook で配送開始・完了を祖先判定の前後へ挟む。実 Git merge を使い、通知の元になる
  integration 行、IntegrationDetected の不在、受信箱項目の消去と追記 event を検査する。
- `bash scripts/dev/test-affected.sh` → exit 0。Rust 6,654 passed（21 skipped）、web 型検査合格、
  vitest 685 passed、node 105 passed、対象 doc test 3 passed（1 ignored）。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/check-adr-numbers.sh` /
  `sh scripts/dev/progress-index.sh --check` / `git diff --check` → exit 0。

## 未解決

なし。本番への反映・本番 DB の変更は行っていない。
web の offline install は jsesc のキャッシュ不足で失敗したため、通常の frozen-lockfile install で補充した。
試験の外部ネットワーク接続は行っていない。

## 本番への移行

config・DB migration は不要。既存の unintegrated は更新後の定期 sweep（300 秒周期）で祖先を確認し、
統合済みに更新する。CoS の知らせは既存の受信箱同期で解消する。

## 提案

なし。
