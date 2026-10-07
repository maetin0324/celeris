---
title: 葉 adr — ADR 2026-10-07-cos-live-fixes と gui-api 節番号の振り直し
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 adr: ADR 2026-10-07-cos-live-fixes と gui-api.md の節番号

## やったこと

- `agent-docs/adr/2026-10-07-cos-live-fixes.md` を書いた（状態: 承認済み・未実装）。
  D1 作成時の `attachment_ids` と同一 transaction の pin（検証失敗なら task も作らない、skill cos-operator §3a の新しい形）、
  D2 `POST /api/v1/knowledge/inbox` と `ALLOWED` の `knowledge.record`、CLI は CoS credential で API 経由、scope は `project:<slug>` に統一、
  D3 `ResolveBody.confidence`（0..=1、任意）を `cos_operations` と event に残す、
  D4 終端済み run を takeover から外す・判定と回収を同じ transaction に・終端記録の後に handle/lease を手放す、
  D5 台本 `scripts/dev/cos-chat-live.sh <repo> <bin dir> <data dir> dry|full` の方針。
- `docs/api/v1/gui-api.md` の重複見出しを出現順に振り直した:
  CoS チャット 3.127→3.128、CoS credential 3.128→3.129、受信箱の一次対応 3.129→3.130、信頼端末 3.128→3.131（3.131.1〜4 も）。
  モデル catalog の 3.127 はそのまま。文書内の §参照（冒頭の override §3.130、route 表 178 行 §3.128、211〜214 行 §3.131.x、
  §3.130 本文の credential 参照 §3.129）と `agent-docs/adr/2026-10-07-browser-trusted-devices.md` の §3.131 を直した。
  `2026-10-06-model-role-assignments.md` の §3.127.4 はモデル catalog を指すので変更なし。過去の progress と crates/ は触っていない。

## 証拠

| コマンド | 結果 |
|---|---|
| `grep '^### 3\.' docs/api/v1/gui-api.md \| awk '{print $2}' \| sort \| uniq -d \| wc -l` | 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（`ok (160 files)`） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `python3 scripts/dev/check-architecture-map.py` | exit 0 |

文書だけの変更なので nextest・clippy は回していない（crates/ の差分なし）。

## 未解決事項

- D4 の根因（handle の終了と終端記録の順序か、triage run の handle の扱いか）は d4-takeover の葉が決定的な試験で確かめる。
  ADR は「終端なら何もしない」「判定と回収を同じ transaction」「終端記録の後に手放す」の 3 つを決めた。

## 提案

- なし
