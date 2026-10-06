---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
---
# Phase 1: model routing の設定と互換 catalog

`[model_routing]` に `mode`（既定 `legacy`、`shadow` 可）、`[[models]]`、`[[deployments]]`、`[policies.<lane>]` を追加した。`enforce` は Phase 2 まで検証エラーにする。model と deployment は別の型で保持し、後者は source と wire model を参照する。資格情報と URL は catalog に写さない。

proxy 由来の値は `llm_proxy::legacy_catalog::normalize_legacy_config` から取得する。`[[providers]]` の `tier_models`、具体 `model`、`llm_source` も正規化する。旧 ID と認証設定は変更しない。新しい同一キーの定義は旧由来値を上書きする。model/deployment の重複、存在しない model/source/deployment 参照、既存 deployment の source/model/wire identity と矛盾する上書きは拒否する。警告は設定キー、旧出自、移行先だけを含み、同じキーを 1 回にまとめる。

`Config::load` は全体検証後に catalog snapshot を確定し、daemon の config に保持する。API 葉に渡せる共有 handle は snapshot 全体を単一の `Arc` として交換し、既に読み始めた呼び手は旧 snapshot を保持できる。reload は新しい設定の検証と catalog 構築が成功してから交換する。不正設定では旧 snapshot と provider 設定を保つ。legacy 実行経路の選択規則は変えない。

検証: `routing_legacy_config_normalizes_with_warnings`、`routing_legacy_equivalence_qwen_is_cheap_only`、`routing_config_reload_is_atomic`。旧本番形の fixture で Qwen の deployment が cheap だけであること、秘密を警告に出さないこと、新旧 identity 衝突と不正 reload を固定した。`cargo test -p celeris --lib -- --quiet`（320 件成功）、`cargo clippy -p celeris --all-targets -- -D warnings`、`cargo fmt --all -- --check`（成功）。
