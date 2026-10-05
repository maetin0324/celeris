---
title: org-tests — browser 実行課の追加に伴う org 試験修正
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
updated: 2026-10-05
---

# org-tests — browser 実行課の追加に伴う org 試験修正

`config/org.example.toml` の browser-execution 追加に合わせ、`crates/celeris` の org 例の id 一覧と seed 件数を 15 に更新した。

matching では実効 profile に `browser-enabled` skill を持つ専用 node を、browser を要求しない task の候補から外す。これにより空 skill の coding task は従来の `cluster-hpc` に割り当たり、browser-execution に流れない。grant だけを持つ兼任 node の通常 task は従来どおり候補になる。この判断を設計 ADR の D1.2 に記録した。

## 検証

- `cargo test -p task-ops matching::tests --lib`: 13 passed、0 failed。`browser_specialist_` 試験で空 skill の task も確認。
- `cargo test -p celeris --lib`: 323 passed、0 failed。元の失敗 3 件を含む。
- `cargo fmt --all -- --check`: exit 0。
