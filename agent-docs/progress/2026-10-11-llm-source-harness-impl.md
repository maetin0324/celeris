---
tasks: [01M4MAYXS5C7438RAB6TRZK60W]
status: running
title: LLM source・harness 実装の進捗
updated: 2026-10-11
---

# LLM source・harness 実装の進捗

## ADR 修正

`agent-docs/adr/2026-10-11-llm-source-harness-selection.md` を人の redesign 決定に合わせて改訂した。

- `worker_hint.adapter` は task 選択経路から除去。既存 task は migration で `harness` 欄へ移し、解決不能なら null と `harness_migration_unresolved` event を記録する。
- `[llm_proxy.models]` の tier alias は移行期間なしで削除し、langmem・LDR・paperqa は具体的な source/model を設定する。
- 通常 task は `[route_defaults.<harness_id>]` の優先順 `candidates` を `default_route_policy` が選ぶ。明示候補が無い場合は fail closed。coding の Claude/Codex、軽量仕事の Qwen を例示した。
- CoS は `claude_code` harness に固定し、`[cos.route_candidates]` の候補を同じ admission で選ぶ。task 01M4E0WA5Z の代替 source fallback を保持する例を追加した。
- lane/Tier は source/model 対応に使わず、選択入力にするかは task 01M4M8D4AQ に委ねる。`RouteRequest.lane` は維持。
- 本番設定はこの実行環境に見つからなかった。model ID は repo の production host 用 config example と model catalog で確認した。

## 確認

以下の文書検査はすべて exit 0。

- `bash scripts/dev/check-doc-links.sh`
- `bash scripts/dev/check-adr-numbers.sh`（182 files）
- `bash scripts/dev/progress-index.sh --check`
- `sh "$CELERIS_WU_SCOPE_PATHS"`（変更は ADR と本進捗ファイルのみ）
- `git diff --cached --check`
