# Browser allowed_domains の origin 土台

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
---

- ADR `2026-10-05-browser-allowed-origins.md` で形式、既定 port、旧 host の狭める読み込み規則を決定。
- task-core に origin の解析・正規化・包含・交差・冗長削除を追加。`EffectiveBrowserPolicy::derive` は grant と task policy を scheme・host・port 込みで交差する。
- org の TOML/JSON seed は loopback の 3000 番 port のみに変更。両 seed を `include_str!` で読む検証試験を追加。
- 実行側の broker/egress と起動引数の配線は後続の enforce 工程が担当する。
- 検査: `cargo test -p task-core --lib` は 723 件成功、`cargo clippy --workspace -- -D warnings` は成功。
