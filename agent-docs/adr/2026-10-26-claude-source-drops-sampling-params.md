# llm-proxy の claude source は sampling 引数（temperature / top_p）を上流へ送らない

- 日付: 2026-10-26
- 状態: 採用・実装済み
- task: 01M5WCJ9ZMY4TFN2P609S6RAMR
- 関連: [ADR-0053](0053-llm-source-proxy.md)（LLM source proxy）、[ADR-0029](0029-web-research-harness.md)（LDR）、
  [破壊的変更の方針](2026-10-11-breaking-changes-by-default.md)

## 背景

2026-10-26、BenchFS の IO500 調査の子 task 01M5WC8PPWXB22788R17Y10DEB（harness local-deep-research、
model `claude_oauth/claude-sonnet-5-5`、llm-proxy 経由）が 3 回とも
`400 - {'error': {'message': '`temperature` is deprecated for this model.', 'type': 'upstream_error'}}` で落ちた。
LDR は LangChain の `ChatOpenAI` に自分の設定 `llm.temperature`（既定 0.7）を渡し、OpenAI 互換要求に
`temperature` を付ける。claude source（`crates/llm-proxy/src/sources/claude.rs`）はそれを Anthropic Messages
API の要求へそのまま写していた。Claude 5 系は sampling 引数（`temperature`、`top_p`）を拒否する。

codex source は Phase 65b（ADR-0053 追記）以降、既定で `temperature` を送らない。

## 決定

- D1. claude source は OpenAI 互換要求の `temperature` と `top_p` を Anthropic 要求へ写さない。値は黙って捨てる
  （client 側に 400 を返さない。LDR・PaperQA など proxy の client は sampling 引数を自前の既定で付けるので、
  proxy が吸収する）。`stop` → `stop_sequences`、`max_tokens` はこれまでどおり写す。
- D2. 送る経路を残す設定（codex の `send_sampling_params` に当たる欄）・model 名による分岐は作らない。
  Claude の旧 model 向けに sampling を効かせる必要が出たら、そのときに別の判断として扱う。
- D3. client 側（LDR adapter の `ldr_run.py`、`local_deep_research.rs`）は temperature を設定しない。
  LDR 自身が付ける `llm.temperature` は proxy が捨てる前提で、adapter の説明にそう書く。
- D4. 試験: `to_anthropic_body` の単体試験と、proxy の統合試験（`proxy_integration.rs`）で
  `temperature`/`top_p` 付きの要求が claude source で 200 になり、上流要求に両方が無いことを固定する。

## 影響

- Claude 4 系以前の model で sampling 引数による出力の揺れの制御はできなくなる（現行の route は Claude 5 系）。
- 本番への反映は release が要る。確認手順は進捗
  [2026-10-26-claude-source-drop-temperature](../progress/2026-10-26-claude-source-drop-temperature.md) に書く。
