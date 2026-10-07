---
title: ops 文書: KB に cos skill を置く手順と CELERIS_API_URL
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-07
---
# ops-docs2 — live-check 不具合 3 の扱い

[docs/ops/cos-chat.md](../../../docs/ops/cos-chat.md) に 3 節を足した。

- CoS skill の配置: `[knowledge] root` の `skills/` に `config/skills/cos-operator`・`cos-inbox-triage` を置く手順、無いと `CoS unavailable: CoS skills unavailable` になること、`test -s` と web の一往復による確認。本番 KB への配置は人の手順。
- CELERIS_API_URL: api-env の結果（CoS run の celerisctl は daemon 自身の API へ送る。優先順 `--api-url` > env > config。staging が本番へ向かわない）。
- 添付の引き継ぎ: attach-handoff の使い方（task への pin→作業 run の入力 manifest、KB inbox candidate）。

## 検証

| コマンド | 結果 |
|---|---|
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 0 |
| `python3 scripts/dev/check-architecture-map.py` | exit 0 |
| `sh scripts/dev/progress-index.sh --check` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0 |

## 未解決事項

なし。
