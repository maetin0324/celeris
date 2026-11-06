---
title: 予算の token 上限を料金換算（USD 相当）の上限に置き換える
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
status: running
updated: 2026-11-06
---
# 予算の token 上限を料金換算（USD 相当）の上限に置き換える

完了日: 未完了（WorkUnit adr まで）

人の決定（2026-11-06）「料金の換算で数える」を実装する task。設計は
[ADR 2026-11-06-budget-usd-caps](../adr/2026-11-06-budget-usd-caps.md)。

## 変更
- WorkUnit adr: ADR `2026-11-06-budget-usd-caps.md` とこの進捗ファイルの骨子を追加した。コードは変えていない。
- 残りの WorkUnit（core・scripts・ops・wire-rest・closeout）は未着手。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- WorkUnit adr は文書だけの変更。cargo の検査は closeout で提出時の HEAD に対して実行して、ここに書く。

## 本番への移行
1. 消す config 欄: `[project_controller.budget] max_tokens_per_day`、`[execution.tree] max_tree_tokens`。新しい binary は、この欄を書いた config を読み込みエラーにする。
2. 新しい欄:
   - `[project_controller.budget] max_usd_per_day`（既定 100.0 USD 相当）
   - `[execution.tree] max_tree_usd`（既定は無し）
   - `[budget_rates."<model の接頭辞>"]`（単価表に無い model の換算単価。4 欄必須。既定は無し）
3. 本番 config の書き換え: 不要の見込み。2026-11-06 時点で、本番 config（`~/.config/celeris/config.toml`）に `max_tokens_per_day`・`max_tree_tokens` は書かれていない（task の依頼文の前提。この run は本番 host を読んでいないので未確認）。人は promote の前に次で確かめる。
   - `grep -nE 'max_tokens_per_day|max_tree_tokens' ~/.config/celeris/config.toml` → 何も出ないこと。出たら同じ値の意図を USD で `max_usd_per_day`・`max_tree_usd` に書き換える。
4. `[project_controller]` を有効にした案件が local model（opencode-qwen など単価表に無い model）を使うなら、promote の前に `[budget_rates]` を書く。書かないと費用不明で日次の予算が止まる（ADR D2）。
5. promote の前に、未回答の木の上限の決定で key に `max_tree_tokens` を含むものが無いことを人が確かめる（新しい binary は旧い種類を読まない）。確かめ方は closeout が API か `celerisctl` のコマンドで書く。
6. 運用中の pilot の手順（`long-horizon-pilot-llm.sh`）は `--max-tokens-per-day` を `--max-usd-per-day` に変える。

## 未解決
- route の `<source>/` 接頭辞付きの model 名が `runs.model` に入るか（ADR D1-6、core WU が確かめる）。
- harness を起動する前に終わった run を usage の欠測から区別する規則（ADR D2-3、core WU が決めて試験する）。

## 提案
- なし。
