---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: B0封印入力と共有digestの接続（campaign-seal）
work_unit: campaign-seal
status: done
updated: 2026-10-19
---
# campaign-seal（propagation §8 提案 P1）

## 変更（campaign crate のみ。逆依存なし）
- `B0Inputs::from_sealed(bytes, approved_sha256)`: 承認済み sha256（64 桁小文字 hex、呼び出し側が明示）と bytes の sha256 が一致するときだけ解釈する。不正形式は `Missing`、不一致は `ContentHash`。
- preflight `Inputs.variant_b0_sha256`（CLI `--variant-b0-sha256`）。variant 参照があるのに未指定、または file 欠落・改変なら `variant` 検査が失敗する。
- `OrgSnapshot` は items を `OrgNode` として読み、`effective_profiles` が `resolve(items)` と一致しない応答を拒否。`digest()` は task-core `org_effective_digest`（共有関数）。
- `check_b0_org`（`check_pin` から呼ぶ）: spec-on は B0 と稼働 org の全 node digest が一致、spec-off は B0 と共通の node（根 cos 必須。coding 課は spec が見る）の profile が一致。
- 公開 VariantRef / spec / manifest / lock / hash の固定値は不変（既存試験が通る）。

## 証拠
- `cargo test -p celeris-campaign` → 全 binary ok（lib 42 passed。新規: from_sealed 正負、全 node digest、cos/research drift、sha 未指定・誤り・大文字・改変）。launcher 試験 1 件が負荷で 1 度だけ落ちたが再実行で通過（本変更と無関係）。
- `bash scripts/dev/test-parallel.sh` → passed 5878 / failed 0、`test-parallel: ok`
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0

## 残る制約
- `Inputs` に欄を足したため `crates/celeris/tests/campaign_lock_e2e.rs` と campaign 試験 4 件のリテラルに `variant_b0_sha256: None` を足した（ビルド成立のため）。
- 実際の承認済み sha256 の値の決定・配布は人の運用手順（本 WU は実 org・実 LLM に触れていない）。
- `cargo clippy -p celeris-campaign --all-targets` は既存の `tests/env_lock.rs`（本変更外）で落ちる。gate の形（`--workspace`）は通る。
- run 単位の観測 digest との照合（L2）は campaign-proof で行う。
