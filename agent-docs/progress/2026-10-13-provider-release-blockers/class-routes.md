---
title: execution class ごとの route 候補 — claude_code 既定 sonnet、coding だけ opus
tasks: [01M4TFTRHVBHP021X7MVJESYCM]
status: complete
completed: 2026-10-13
---

# execution class ごとの route 候補 — claude_code 既定 sonnet、coding だけ opus

親は provider 廃止リリース不具合 task（`01M4T94XWN`）の段 fix。ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の 2026-10-13 付記 §5 に従う。人の決定（2026-10-13）: claude_code の既定 model は `claude-sonnet-5-5`、execution class `coding` だけ `claude-opus-5-5`。本番 config には触れていない。

## 各 leaf の要点（`class-routes/` のメモ）

- `core-dispatch`（[class-routes/core-dispatch.md](class-routes/core-dispatch.md)）: `Registry.route_class_defaults` を足し、`route_priority` が execution class の列を先に見るようにした。class の列が宣言されていればそれで `route_defaults` を置き換え（fallback しない）、無ければ harness 既定。`route_select` は class を harness loop の先まで届ける。CoS は class に依らない。
- `config`（[class-routes/config.md](class-routes/config.md)）: `[route_defaults.<h>].classes.<class>.candidates` を読み、検証（class 名は coding / planner / review のみ、空の列は拒否、harness 既定と同じ source・model・proxy 検査）、`routing show` に `<harness> class <class>: <source>/<model>` を出す。
- `runbook`（この記録の後段）: 移行 runbook の TOML と選択結果の表、progress 写し、`migration_runbook_toml_loads` の期待値を揃えた。

## この leaf の変更

- `docs/ops/provider-llm-source-migration.md`（migration-runbook-toml の marker 間）:
  - `[route_defaults.claude_code].candidates` の先頭を `claude-sonnet-5-5` にした。並びは sonnet → opus → fable（sonnet の重複を避けるため既存の sonnet 行を前へ移した）。
  - `[route_defaults.claude_code.classes.coding].candidates = [claude_oauth / claude-opus-5-5]` を足した。
  - 選択結果の表: coding worker は opus（class 候補は opus だけ。admission 不可なら claude_code は飛ばされて Codex `gpt-6.1-sol`）、planner・reviewer は sonnet 先頭、「その他の claude_code 仕事」行を追加、CoS は opus 先頭・Codex astra 次（変更なし）。
  - 手順 6 の期待: `claude_code` の先頭は sonnet（coding class は opus）。
- `agent-docs/progress/2026-10-11-provider-layer-redesign.md`: 同じ marker 間の TOML の写し（同じ 2 行の差し替えと coding class の 2 行）。runbook の marker 間 TOML と一字一句一致を確認（`str.contains` 相当で一致）。
- `crates/celeris/src/config/tests.rs` `migration_runbook_toml_loads`:
  - 既存の期待値 `("claude_code", "claude_oauth", "claude-opus-5-5")` を `claude-sonnet-5-5` に直した。
  - `registry.route_class_defaults[claude_code][coding][0].model == "claude-opus-5-5"` を assert に足した。

## 証拠コマンドと結果

- `cargo test -p celeris --lib migration_runbook_toml_loads` — exit 0。`config::tests::migration_runbook_toml_loads ... ok`、1 passed, 0 failed。
- `cargo test -p celeris --lib config::` — exit 0。143 passed, 0 failed（class 候補の試験 `claude_code_class_route_override_picks_opus_for_coding_only`、`class_candidates_are_validated_like_harness_candidates` などを含む）。
- `cargo clippy --workspace -- -D warnings` — exit 0。`Finished dev profile`、警告なし。
- `rustfmt --edition 2024 --check crates/celeris/src/config/tests.rs` — exit 0。
- runbook と progress の写しの一致: runbook の marker 間（前後の ` ```toml ` 囲みを除く）が progress の文字列に含まれることを python で確認（True）。
- 範囲: `git diff --stat` は 3 file（`docs/ops/provider-llm-source-migration.md`、`agent-docs/progress/2026-10-11-provider-layer-redesign.md`、`crates/celeris/src/config/tests.rs`）と、この記録ファイル・`class-routes/` の leaf メモ。

## 本番への移行（人が実行する手順。計画の decisions で扱う）

本番 config は本 WU では触っていない。移行時は runbook の「切替前の退避と確認」3 のとおり、新 binary で作業用コピーを検査してから本番 path に置く。その検査で class 候補が効いていることを確かめる:

```sh
new_ctl=~/.local/celeris/releases/<新リリース>/bin/celerisctl
"$new_ctl" routing show --config "<作業用コピーの config.toml>" | grep -E 'claude_code( class coding)?:'
```

期待: `claude_code:` の行の先頭候補が `claude_oauth/claude-sonnet-5-5`（候補は ` > ` で続く）、`claude_code class coding:` の行が `claude_oauth/claude-opus-5-5` だけを示す。`claude_code class coding` の行が無ければ、class 欄が読まれていない（config の `classes` 欄の綴り違いは読み込みエラーになる）。reload 前にこの確認を通すこと。

## 未解決事項

- daemon の harness API（`daemon/api.rs` の `route_defaults`）は class 候補をまだ出さない（config leaf の注意に同じ）。runbook 手順 6 の `GET /api/v1/harnesses` の jq では coding class の行は見えない。確認は `routing show` で行う。
- coding class の列は opus 1 件だけにした。opus が使えないとき sonnet へは落ちない（claude_code が飛ばされ Codex へ行く）。sonnet も候補に入れるかは人の判断が要る。
- `paperqa` の `claude_oauth / claude-opus-5-5` は harness 既定のままにした（本件は claude_code の既定だけが対象）。

## 提案

- sonnet を coding の 2 番目に入れるか（opus の上限に当たった時に claude_code を生かす）は、実運用の admission 記録を見てから決める。
- 選択の理由（class 候補のどれが選ばれたか）を `routing_decided` の metadata に入れると、本番確認が DB を読むだけで済む（config・core-dispatch の提案と同じ）。
