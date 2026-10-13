---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: config 例を新形にし config_examples_load を足す
updated: 2026-10-11
---
# config 例を新形にし config_examples_load を足す

## 範囲と変更

- `config/*.example.toml` と `config/{paperqa.proxy,opencode.openai-compat}.example.json`: `[[providers]]` は残っていない（grep で 0 件）。
  model 名の tier alias（`celeris/cheap` など）は具体的な `source` / `model` に置き換え済み（既存の作業分を検収）。
- `config/celeris.example.toml`: 配置の説明から `providers.d/` を外した。`[cos]` は `route_candidates = []`（空なら理由付き unavailable）。
- `crates/celeris/src/config/tests.rs`: `config_examples_load` を追加。`config/*.example.toml`（`org.example.toml` を除く 12 件）を
  `Config::load` して `llm_route_registry()` が通ることを確かめる（件数 10 以上も確かめる）。例の provider 前提の試験は新形の検査へ書き換えた。
- `crates/celeris/src/config/{dispatch,providers}.rs`: `[[providers]]` が空でも `[llm_sources.<id>]` があれば読めるようにした（旧い検査は provider 行と一緒に消す）。
- `docs/ops/opencode-go-and-model-catalog.md`: config 例を `[llm_sources.opencode_go]` / `[harnesses.opencode]` / `[route_defaults.opencode]` に、同時実行数と model の説明を新形に。
- `docs/ops/coding-harness-pi-hashline.md`: config の追記と戻し方を `[llm_sources.opencode_go]` / `[harnesses.pi]` / `[route_defaults.pi]` / `[harness_selection]` に。
- `docs/ops/cos-chat.md`: `[cos]` 例を `route_candidates`（`source` + `model`）に。`[[cos.fallbacks]]` と `provider` の説明を削除。

## 実行したコマンドと結果

- `cargo test -p celeris --lib config_examples_load` → 1 passed（exit 0）。
- `TMPDIR=/tmp cargo test -p celeris` → 全 crate 合格、celeris lib 419 passed / 0 failed（exit 0）。
  - 注: 既定の TMPDIR（run の長い path）では `browser_doctor` の 2 件が Unix socket の path 長上限で落ちる（`UnixListener::bind` の unwrap）。HEAD でも同じ。試験 fixture に限り `TMPDIR=/tmp` で通る。
- `cargo clippy -p celeris --all-targets -- -D warnings` → Finished、警告 0（exit 0）。
- `cargo fmt -p celeris -- --check` → 差分なし（exit 0）。

## 未解決（この葉の範囲外。後の段で扱う）

- `[[roles]]` / `[[genres]]` / `[[harnesses]]`（組織に見せる実行契約）の `tier` と `adapter` 欄は、examples に残してある。
  読み込みは旧い互換経路で、tier 焼き付けと worker_hint.adapter を消す段（hint-remove・config-remove）で消す。
- `config/celeris.example.toml` の `[reviewer] adapter` / `tier` と `[conversation] genre`、`[[harnesses]]` の見本は旧い形のまま。
- 同じキー `harnesses` を `[[harnesses]]`（組織）と `[harnesses.<id>]`（実行器）で併用できない。例は `[harnesses.*]` を
  コメントアウトしたまま（`celeris.example.toml` 内の注記。決定待ち）。
- harness 宣言に `env` 欄が無い。`celeris.opencode-go.example.toml` の `OPENCODE_DISABLE_PROJECT_CONFIG` は例から外した。
  必要なら harness 宣言に欄を足す決定が要る。
- `docs/ops/coding-harness-pi-hashline.md` の §3 の `[[harnesses]] coding` の `adapter_policy` 節と、
  `docs/ops/opencode-go-and-model-catalog.md` の §3a（`POST /api/v1/providers`・tier 割り当て）は旧い仕組みの説明のまま。
  `/api/v1/providers` の削除（api-remove）と adapter hint の削除（hint-remove）の段で書き直す。
- `docs/ops/model-routing-migration.md`・`provider-llm-source-migration.md`・`selfdeploy.md` は移行の記録で、この葉では触らなかった。

## 本番への移行

- 本番の `~/.config/celeris/config.toml` は、この葉では変更しない。旧い `[[providers]]` の config は config-remove の段で読み込みエラーになる。
  その段の進捗ファイルの「本番への移行」節に、移す手順（`[[providers]]` → `[llm_sources]` / `[harnesses]` / `[route_defaults]`）を書く。
