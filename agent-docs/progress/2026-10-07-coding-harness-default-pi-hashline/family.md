---
title: task-core の model family 判定と coding 既定 adapter
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
status: done
updated: 2026-10-07
---
# task-core の model family 判定と coding 既定 adapter

WorkUnit `family`。ADR `agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md` の純粋関数を実装した。

## 実装

- `model_family.rs`: `ModelFamily`、`FamilyBasis`、`FamilyDecision`、`derive_family`。型付き LLM source → account pool → `ModelProfile.family` の順で導出する。provider 名、relay 識別子、model id の文字列は判定に使わない。Unknown / Other は非 Claude。
- `coding_harness.rs`: `CodingHarness::for_family` / `adapter_id`、`prefers`、`coding_harness_default_adapter`。Claude は `AccountAdapter::ClaudeCode.as_str()`、それ以外は `pi`。明示 adapter は加工せずそのまま返す。指定の検証は呼び出し元の責務。
- 後続の設定・監査用に `AdapterPolicy`（既定 ProviderOrder）と `AdapterChoice` も公開した。
- `ModelProfile.family: String` は互換性のため維持。新しい型は既存 API / protocol の schema root に未接続なので、この葉で schema 再生成は不要。

## ADR との差分と後続への引き継ぎ

`derive_family(source, pool, model: Option<&ModelProfile>)` は catalog が無い行にも使える。`DeploymentProfile` は family を持たず、ADR でも source_ref の文字列を判定から除外しているため、未使用の deployment 引数は設けなかった。

この葉の Objective は task-core の判定・純粋関数に限定される。ADR の `HarnessSpec.adapter_policy` 欄追加、config の読み込み、既存 Qwen 判定の置き換えは後続の dispatch 配線で実施が必要。`crates/celeris/src/config/harness.rs` は並行 pi-adapter 葉の対象なので変更していない。dispatcher の適用・Pi worker の起動・本番有効化はこの葉では未実施。

## 検証

- `cargo test -p task-core coding_harness_default`: 必須6ケース（Claude / GPT・OpenAI / OpenCode Go / DeepSeek / 未知 / 明示指定）と parse・優先順位・preferred・serde の10試験。
- `cargo test -p task-core`: 782 unit + 1 integration、失敗0。既存試験と schema snapshot の整合を確認。
- `cargo fmt --all --check`、`cargo clippy -p task-core --all-targets -- -D warnings`: exit 0。
- 文書リンク・progress-index・`CELERIS_WU_BASE` 基準の差分範囲検査: exit 0。実行結果は run の `result.json` にも記録した。
