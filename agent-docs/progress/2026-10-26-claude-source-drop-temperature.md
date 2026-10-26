---
title: llm-proxy の claude source が temperature / top_p を上流へ送らない
tasks: [01M5WCJ9ZMY4TFN2P609S6RAMR]
status: done
updated: 2026-10-26
---
# llm-proxy の claude source が temperature / top_p を上流へ送らない

完了日: 2026-10-26

判断は [ADR 2026-10-26-claude-source-drops-sampling-params](../adr/2026-10-26-claude-source-drops-sampling-params.md)。

## 変更
- `llm-proxy: claude source が temperature / top_p を上流へ送らない` —
  `crates/llm-proxy/src/sources/claude.rs` の `to_anthropic_body` から `temperature`・`top_p` の写しを削除
  （送る経路を残す flag・model 分岐は無し）。統合試験 `claude_drops_sampling_params_from_the_upstream_request`
  を追加し、偽の Anthropic 上流を Claude 5 系と同じく sampling 引数で 400 を返す形にした。LDR adapter
  （`local_deep_research.rs`、`local_deep_research_run.py`）と `docs/guides/llm-source.md` に、temperature は
  proxy が吸収する前提の説明を足した。adapter 自身は temperature を設定していなかった（LDR が LangChain 経由で
  自前の `llm.temperature` 既定値を付ける）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p llm-proxy --test proxy_integration` → 25 passed。
- 回帰の確認: `to_anthropic_body` に temperature の写しを一時的に戻すと新しい試験が 1 failed（400）になり、
  戻すと pass。
- `bash scripts/dev/test-affected.sh` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check`
  → 結果は task の完了報告に記す。

## 本番での確認手順（operator。release 後）
1. この変更を含む release を promote する（通常の `scripts/selfdeploy/release.sh` → promote）。llm-proxy は
   daemon に内蔵なので daemon の差し替えで反映される。
2. 子 task 01M5WC8PPWXB22788R17Y10DEB を reopen する（GUI の task 詳細の「再開」、または
   `POST /api/v1/tasks/01M5WC8PPWXB22788R17Y10DEB/reopen`（admin token、body `{"expected_status":"failed"}`））。
3. 新しい run（harness local-deep-research、model `claude_oauth/claude-sonnet-5-5`）が exit 0 で終わり、
   stderr に `` `temperature` is deprecated `` が無いことを run の log で見る。
4. 本番 DB を read-only で開き、proxy の記録が ok になっていることを見る:
   `sqlite3 -readonly <本番 DB> "SELECT status, count(*) FROM llm_proxy_requests WHERE task_id='01M5WC8PPWXB22788R17Y10DEB' AND ts > strftime('%s','now','-1 day') GROUP BY status;"`
   → `ok` だけ（`error` が出たら `error_kind` を見る。task_id が空の行は ts と upstream_model で絞る）。
5. report.md・sources.json が artifacts に出て、親 task の review に進むことを確かめる。

## 未解決
- codex source には `send_sampling_params`（opt-in で temperature を送る）が残っている。本 task の範囲外なので
  触っていない。

## 提案
- codex の `send_sampling_params` も実際に使っている設定が無ければ消し、両 source の方針を「sampling 引数は送らない」
  に一本化する。
