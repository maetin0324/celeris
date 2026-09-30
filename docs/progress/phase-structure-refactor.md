---
tasks: [01M3RCEF8QZV26GY3EYEDRRZ5T]
---

# 構造リファクタリング: inline test 外出しと module 境界

## 最終状態（2026-09-30）

前後 LOC、残存 2k 行超 production の分類、module map、互換性差分の確認は [最終計測レポート](/var/lib/celeris/workspaces/01M3Q6F0Y8M0HDMF6Y68G8519M/wu/final/artifacts/report.md) を参照。

- Rust inline test は開始監査 100,831 行（Rust test 全体の 68%）から、最終 HEAD 15,673 行（10.7%）になった。大きな test 群を隣接する `tests.rs` へ移し、private item へのアクセスを維持した。
- config、dispatcher、store、handlers、daemon 起動配線、execution plan、worker/ops の責務境界を crate 内 module に整理した。公開 crate 境界と既存 API を保ち、不要な trait や crate 分割は導入していない。
- `docs/api/v1`・`docs/protocol` schema、`config/` example、task-core migrations の開始 HEAD からの差分はゼロ。
- 2k 行を超える production は `paperqa.rs`、`dispatcher.rs` facade、`task-api/types.rs` の 3 ファイル。分類と残す理由はレポートに記載した。

監査からの段階分け: 巨大 inline test の移動を先に行い、P0 で config / dispatcher / store、P1 で GUI task detail / API handlers / celeris daemon、P2 で task-core / worker / ops の cohesion を扱った。責務の移動と関数本体の再設計を分離し、小さくレビューできる統合単位にした。

検査記録: 各段階の `cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings` の結果は各 WorkUnit の記録にある。最終 HEAD は warning-only source size guardrail を含む。
