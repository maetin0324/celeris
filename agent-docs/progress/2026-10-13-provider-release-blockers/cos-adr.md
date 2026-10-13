---
task: 01M4T94XWN
wu: cos-adr
decision: cos-codex (a) CoS は Claude のみ
status: done
date: 2026-10-13
---

# cos-adr: ADR 本文の CoS 記述を Claude のみに書き換える

## 変更箇所

1. `agent-docs/adr/2026-10-11-llm-source-harness-selection.md`
   - 「## CoS の経路」（156 行付近）: 本文の Codex fallback の文を消し、CoS の候補を `claude_oauth / claude-opus-5-5` の 1 件だけにした。Codex を CoS 候補に置かない理由（`claude_code` は proxy endpoint を宣言できない）を 1 文で書いた。TOML 例の `route_candidates` も 1 件にした。
   - 「実装突き合わせ付記（2026-10-12、本番移行 runbook）」（318 行付近）: CoS の文を「standard の `claude-opus-5-5` の 1 件だけ（Codex へは落とさない）」に直した。
   - 同じ付記の検証の記述（328 行付近）: 「CoS の候補順」を「CoS の候補（Claude の 1 件）」に直した。
   - 「2026-10-13 付記（CoS の Codex fallback 撤去 — 決定 cos-codex）」§1 （451 行付近）: 「既存の文は書き換えず」の句と、旧文の引用を消し、「本文（CoS の経路・runbook 付記）を本付記に合わせて書き換えた」に直した。
2. `agent-docs/progress/2026-10-11-provider-layer-redesign.md`
   - 「選択結果」表の CoS 行（142 行）: `codex_oauth / gpt-6-astra` の fallback を消し、「fallback なし（候補は Claude の 1 件。不適格なら fail closed。Codex へは落とさない）」にした。
   - 同ファイルの TOML（`[cos]` 区間）は既に Claude の 1 件（align-cos で揃え済み）のため変更なし。

## 変更しなかった箇所（対象外）

- planner・reviewer・coding の Codex 候補（2026-10-12 付記、選択規則の表）
- PaperQA・LDR・LangMem の Codex 候補（同上）
- 2026-10-13 付記 §3 の「CoS は Claude source のみとする」（決定と同内容）
- `docs/ops/provider-llm-source-migration.md`: CoS 行と確認手順は既に Claude の 1 件（align-cos で揃え済み）。残りの CoS×Codex の記述なし。
- コードは変更していない。

## 証拠

- `grep -n -i 'cos' <ADR> | grep -i codex` の残りは 156 行（Codex を置かない理由）と 318 行（「Codex へは落とさない」）、416 行（「Codex OAuth proxy fallback を候補から外し」）、448–458 行（2026-10-13 付記の決定と戻す条件）のみ。CoS の候補に Codex を置く記述は無い。
- `grep -n 'gpt-6-astra\|fallback' <ADR> <runbook> <progress>` で CoS に結び付く残りは無い（PaperQA・LDR の 324・408 行は対象外の role）。
- `cargo nextest run -p celeris -E 'test(=config::tests::migration_runbook_toml_loads)'` → `1 test run: 1 passed, 495 skipped`、exit 0（acceptance 2）。
- 進捗 2026-10-11 の CoS 行（142 行）: Claude のみ（acceptance 1）。

## 未解決事項

- なし。USERNS=1 を含む全体検査は closeout5 の担当。
