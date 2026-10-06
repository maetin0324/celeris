---
title: CoS チャットの運用文書と仕様・構成図の更新
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-06
---
# CoS チャットホーム — ops-docs WorkUnit

## 完了内容

[運用手順](../../../docs/ops/cos-chat.md) に `[cos]` の harness・LLM source・model/tier・account、受信箱 policy、添付の保存場所・上限・保持と GC、CoS の権限・監査、本番の release/verify/promote と rollback、停止・session rollover・障害確認、受信箱一次対応の運用確認（代答・Discord escalation と `gui_base_url` 未設定時の未達・CoS 不在の退避・取消/差し戻し・通知一本化の確かめ方）を記載した。本番変更は実行せず、人が実施する手順と確認方法として書いた。promote は現行 selfdeploy runbook の「人だけ」を守り、CoS の standing authorization が別途明示された範囲だけを許す。

[SPEC](../../../docs/SPEC.md) の CoS・Console・通知・ホーム画面の記述を ADR D1〜D6 に合わせた。[architecture-map](../../../docs/architecture-map.md) に CoS config と GC を追加し、既存 CoS 行の省略パスを検査可能な実在パスへ直した。

## 検証

| コマンド | 結果 |
|---|---|
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 0 |
| `python3 scripts/dev/check-architecture-map.py` | exit 0、294 パス |
| `sh scripts/dev/progress-index.sh --check` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0 |
| `git diff --check` | exit 0 |

文書のみの WorkUnit なので Rust と web の実行試験は担当段の記録を参照する。CoS の実機確認と web UI は並行 WorkUnit の担当。

## 未解決事項

- 計画の check 1 は `sh scripts/dev/check-doc-layout.sh` を引数なしで呼ぶ。この script は TSV 引数が必須で、引数なしだと usage を出して必ず exit 1 になる（文書の内容とは関係なし）。`scripts/dev/check-doc-layout.sh` はこの WU の範囲外なので直していない。

## 提案

- check 1 を `... && sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` に直す（引数付きなら exit 0）。
