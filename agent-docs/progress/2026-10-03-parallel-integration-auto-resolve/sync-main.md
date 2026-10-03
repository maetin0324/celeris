---
title: main（agent-docs 再配置後）の取り込みと ADR の日付名への改名
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# main（agent-docs 再配置後）の取り込みと ADR の日付名への改名

merged-main: af71d4e9d0e2705e692d644564bcd676b83f0e3e

## 衝突の解き方

- `config/celeris.example.toml`: この branch の `[delivery.auto_resolve]` 欄の説明の後に main の ADR-0136 `[storage]` 欄を置き、`[api]` の見出し行は main 側（`docs/api/v1/gui-api.md` への参照）を採った。
- `crates/celeris/src/config/tests.rs`: この branch の delivery 設定の試験 2 件と、main の ADR-0136（hot path・mount 検査）・ADR-0139（langmem の bearer・verify の proxy 不 bind）の試験を両方残した。
- `docs/adr/0137-parallel-integration-auto-resolve.md` を `agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md` へ git mv（ADR-0128 D5: 0128 超えの番号は新設しない）。crates/・config/・scripts/ の `ADR-0137` 参照は `ADR 2026-10-02-parallel-integration-auto-resolve` へ置き換え、renumber 試験の fixture の番号は 0021/0022 にした。main 側の ADR・migration の番号は変えていない。
- `.gitattributes` の union は `agent-docs/PROGRESS.md`・`agent-docs/progress/*.md`・`agent-docs/progress/**/*.md` に替え、旧 `docs/` の行は消した。`scripts/dev/tests/progress_union_merge.sh` は新パス（入れ子を含む）で確かめる。
