---
title: coding harness 既定選択の ADR（Claude 系は Claude Code、非 Claude は Pi + Hashline）
tasks: [01M49XA3X50V0R2YFTQRR9E8D5]
status: done
updated: 2026-10-07
---
# coding harness 既定選択の ADR（adr）

WorkUnit `adr`。コードは変更していない。入力は同じディレクトリの `survey.md`。

## 完了
- 2026-10-07: `agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md` を追加。節: 決定点 / Claude family 判定 / Pi 起動形と tool 集合 / fallback・account pool・cheap lane / metrics 整合 / 後続葉 / Pi/Hashline の host 上の有無。
- 要点: 明示 adapter > 既定解決。既定解決は `HarnessSpec.adapter_policy = model_family` の harness だけ（有効化スイッチ）。task-core に `ModelFamily`（parse が唯一の文字列→enum 変換、未知は非 Claude）と `derive_family`（LlmSourceRef → AccountAdapter → ModelProfile.family）。dispatch は `select_provider_excluding` で preferred 行に絞り、全滅なら従来候補へ fallback。Pi は adapter id `pi`、`--mode json -p`、`--no-extensions` + `-e <Hashline>`、`--tools` allowlist（ACP は無い）。execution metrics には adapter を足さず、routing audit に `family`・`adapter_choice` を足す。
- 追加確認（読み取りのみ）: 同梱 pi-ai の provider に `anthropic`・`openai-codex`・`opencode-go` がある。資格情報は `PI_CODING_AGENT_DIR/auth.json`（docs/sdk.md:414-421）。

## 証拠
- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0
- `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (150 files)`、exit 0
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、exit 0
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0

## 未解決事項
- Hashline の配布形態・tool 名（host に無い）、本番の pi の置き場、`--mode json` の終端 event と usage、codex OAuth の Pi への受け渡し。ADR「未確認点」に列挙。

## 提案
- 有効化（config の `adapter_policy = "model_family"`）は ops-docs 葉の後に人が行う。codex 行が fallback 専用になり codex pool の消費が減ることを人が了承してから。
