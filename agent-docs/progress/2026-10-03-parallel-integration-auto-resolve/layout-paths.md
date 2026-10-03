---
title: auto_resolve の記録・ADR の対象パスを agent-docs 配置と ADR-0128 D5 に合わせる
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# auto_resolve の記録・ADR の対象パスを agent-docs 配置と ADR-0128 D5 に合わせる

完了日: 2026-10-03（WU layout-paths、基点 234162a7）

## 変更

- `classify.rs`: Record は `agent-docs/progress/**/*.md`・`agent-docs/PROGRESS.md`（旧 `docs/PROGRESS.md`・`docs/progress/` も残す）。ADR は `docs/adr/` と `agent-docs/adr/` を同じ名前空間で見る（番号付き・日付名とも Adr）。
- `renumber.rs`: 番号付き ADR の重複・add/add は番号を振り直さず、取り込み側の file を `agent-docs/adr/<追加 commit の日付>-<slug>.md` へ移す。参照追従は取り込み側が足した・変えた file だけ。日付名同士の衝突は NeedsHuman。migration は従来どおり（main の番号は動かさない）。
- ADR `agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md` に節『付記（agent-docs 配置への追従）』。

## 証拠

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- `cargo test -p task-dispatch --lib` → 543 passed / 0 failed
- `cargo test -p task-ops` → 436 passed / 0 failed
- `sh scripts/dev/check-adr-numbers.sh`・`check-doc-links.sh`・`python3 scripts/dev/check-architecture-map.py` → exit 0
- workspace 全体の `cargo test --workspace` / `cargo clippy --workspace` は close 葉で回す。

## 未解決事項

- 番号だけの参照（`ADR-0140` など）は移動後も人に回す。番号付き ADR を本文で自称する branch ではほぼ人回しになる。

## 提案

- なし。
