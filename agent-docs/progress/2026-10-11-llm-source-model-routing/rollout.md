---
unit: rollout
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: done
completed: 2026-10-14
---
# rollout: 本番への移行手順

task の進捗 [2026-10-11-llm-source-model-routing.md](../2026-10-11-llm-source-model-routing.md) の「本番への移行」節を書いた（§1〜§9: 根拠の立った組と規則が選ぶもの、enforce の前提、enforce 用の config 差分、根拠の無い間の route_defaults と lane の既定の差分、切替手順、確認方法、止める条件と戻し方、根拠の立たない genre と足すもの、学習データ export）。
「未解決」「提案」を整理し、ADR 2 本（主 ADR・task-routing-features）の Status を「実装済み」にして未解決を更新した。コードは変えていない。

## 確かめたこと（読み取りのみ、2026-10-14）

- 本番 release `64158056`（= main HEAD）、DB migration 78、`routing_stage`・`routing_evidence` は 0 行、`route_decisions` は shadow 124・eval 307、`route_outcomes` は eval の 271 行だけ。
- 本番 config: `[model_routing]` 無し、`[route_defaults.claude_code.classes.coding] = [opus]`、lane 列と `[route_selection]` は無い。
- 選択器の順位（`route_select/policy.rs` の `compare_candidates`）を根拠表の値に当てた結果: claude_code は sonnet（USD 0.270 < 0.757）、codex は luna（壁時計 76 s。quota・USD が無い）。
- 根拠表の鍵の綴り: runner の候補表（main も）が `claude-code`／`claude-oauth`、選択器・段階 API は `claude_code` で完全一致 → enforce の前提として follow-up に出した。

## 未解決

- 綴りの修正と本番 run の outcome 記録は follow-up task（`followups.json`）。enforce の前提は綴りの修正と人の決定（§3 の class 列の変更）。outcome の記録は学習データのためで enforce の前提ではない。
