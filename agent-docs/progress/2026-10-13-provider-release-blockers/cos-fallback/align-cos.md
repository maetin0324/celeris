---
task: 01M4T94XWN
wu: align-cos
decision: cos-codex (a) Claude のみ
status: done
date: 2026-10-13
---

# align-cos: CoS の Codex fallback を外す（決定 cos-codex (a)）

## 変更点

1. `docs/ops/provider-llm-source-migration.md`
   - `migration-runbook-toml` 区間から `[[llm_routes]]`（harness = `claude_code`、source = `codex_oauth`、model = `gpt-6-astra`）を削除。他 harness の `codex_oauth` route と `route_defaults.codex` は残した。
   - `[cos].route_candidates` を `claude_oauth / claude-opus-5-5` の 1 件だけにした。
   - 「選択結果」表の CoS 行を Claude のみ（Codex へは落とさない、不適格なら fail closed）にした。
   - 候補の説明文（CoS は Claude（Opus）の 1 件だけ、fail closed）を直した。
   - 「読み込み・切替・確認」手順 3 の CoS 記述を `claude_oauth / claude-opus-5-5` の 1 件だけに直した。
2. `agent-docs/progress/2026-10-11-provider-layer-redesign.md`
   - runbook の区間と同じ TOML に揃えた（`migration-runbook-toml` の抽出結果を旧ブロックと差し替え。旧ブロックは 1 箇所だけ存在することを確認してから置換）。
3. `crates/celeris/src/config/tests.rs::migration_runbook_toml_loads`
   - `cos_route_candidates[1]` の 2 assert を削除。
   - 追加: `assert_eq!(registry.cos_route_candidates.len(), 1);`、`assert_eq!(registry.cos_route_candidates[0].source.0, "claude_oauth");`。
   - 既存の `assert_eq!(registry.cos_route_candidates[0].model.0, "claude-opus-5-5");` は残した。

## 実行したコマンドと結果

- `cargo nextest run -p celeris -E 'test(/migration_runbook_toml_loads$/)'`
  - exit 0。`Summary: 1 test run: 1 passed, 494 skipped`。
- `sed -n '/^\[cos\]/,/^max_turns/p' docs/ops/provider-llm-source-migration.md`
  - `route_candidates` は `claude_oauth / claude-opus-5-5` の 1 件だけ。
- `sed -n '/^\[\[llm_routes\]\]/,/^$/p' <runbook と進捗 TOML> | grep -c 'harness = "claude_code"'`
  - 両 file とも 0。

## 未解決事項

- `agent-docs/progress/2026-10-11-provider-layer-redesign.md` の本文 142 行目（CoS 行「`codex_oauth / gpt-6-astra` を fallback」）と、同 file 内の他の記述は今回の範囲外として触っていない。TOML 区間だけを揃えた。
- ADR `2026-10-11-llm-source-harness-selection.md` の付記、`2026-10-13-provider-release-blockers.md` の進捗、USERNS 版を含む全体検査は別 WU（adr-note・closeout）の担当。

## 提案

- 2026-10-11 redesign 文書の CoS 行を次の整理で合わせる。本文の表を書き換えるか、2026-10-13 付記で置き換えを明記するかは、closeout の担当が決める。
