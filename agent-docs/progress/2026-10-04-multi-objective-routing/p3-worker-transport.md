---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: worker-transport
status: complete
completed: 2026-10-05
---
# Phase 3: task-worker の routing context 搬送

## 実装

- `RunContext.context_ref: Option<String>` を追加した。旧 run JSON は `None` として復号し、未知欄は従来どおり無視する。ディスパッチャの全欄 literal には `None` を足した。実際の参照発行と設定は `dispatch-context` の担当である。
- ACP/OpenCode は選択 model が `celeris/frontier|standard|cheap` で、設定された provider が HTTP(S) の baseURL を持つ場合、`OPENCODE_CONFIG_CONTENT` の provider options headers に `x-celeris-routing-context` を入れる。既存 inline 設定の同名 header は大文字小文字を問わず上書きし、他の provider には追加しない。OpenCode の [provider headers](https://opencode.ai/docs/providers) と [inline config の優先順位](https://opencode.ai/docs/config) に沿う。
- aider は `openai/celeris/frontier|standard|cheap` と OpenAI 互換 base URL の組に限り、run 専用 model settings の `extra_params.extra_headers` で渡す。[aider の model settings](https://aider.chat/docs/config/adv-model-settings.html) に沿う。設定ファイルは 0600 の一時ファイルで、run 終了時に削除する。既存の `--model-settings-file` と衝突する場合は unsupported とする。
- claude_code・codex など header 設定を持たない経路、および対象外の ACP/aider は `runs/<run_id>/context-transport.json` に `context_transport=unsupported` を記録する。搬送経路は `header` とする。証跡は `RunTransportEvidence.context_transport: Option<ContextTransport>` として旧記録・未知欄を復号できる。証跡には参照値、credential、prompt を入れず、`request.json` の参照値も除去する。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-worker routing_context_ref_propagates --lib` | 2 passed。偽 ACP adapter に渡る header、同名の偽装値の上書き、対象外 model の unsupported、偽 aider の model settings を確認 |
| `cargo test -p task-worker --lib --quiet` | 769 passed、4 ignored |
| `cargo test -p task-worker --quiet` | 成功。lib 769 passed、4 ignored。統合試験と doc test も成功 |
| `cargo clippy -p task-worker --all-targets -- -D warnings` | 成功 |
| `cargo clippy --workspace -- -D warnings` | 成功 |
| `UPDATE_SCHEMA=1 cargo test -p task-worker protocol::tests::committed_schema_matches_generated --lib` | 成功。worker protocol schema を再生成 |

## 未解決と提案

- 参照の発行・有効期限と proxy での検証は別 WorkUnit の実装であり、この葉だけでは end-to-end の実 HTTP 経路は成立しない。統合段で偽 proxy を使い run から HTTP 要求まで確認する。
- OpenCode の model が daemon から明示されず、設定ファイルが JSONC で JSON として読めない場合は unsupported と記録する。明示 model がある場合は既定の OpenCode 設定から provider が読み込まれる経路も搬送する。必要なら provider identity と baseURL を daemon から構造化して渡し、設定ファイル構文に依存しない経路へ拡張する。
