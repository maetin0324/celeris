---
title: web UI/UX task に main を取り込む（no-ff merge と文書の衝突）
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
status: done
updated: 2026-10-04
---

# PROGRESS — web UI/UX task の main 取り込み

## merge（2026-10-04）

- 取り込んだ main: `c1b24fb6fa8cdd6b3d13a925545a1def1db03bd3`（分岐点 `7f3482a3`、task 側 `f157bc6e`）。merge commit `6f7c4c3b`（no-ff、rebase なし）。
- 衝突 (a): main が `docs/web/` を `agent-docs/web/` へ移したため、task 側で足した web ADR-W4 は旧 path `docs/web/adr/web-0004-design-system.md` から [agent-docs/web/adr/web-0004-design-system.md](../web/adr/web-0004-design-system.md) に置いた（本文は task 側のまま）。旧 path は無い。
- 衝突 (b): [ADR-0081](../adr/0081-web-spa-frontend.md) は main の付記（2026-10-02 gateway の dotfiles と release 追従）と task 側の付記（2026-10-03/04 遷移 latency の測り方）を日付順に両方残した。task 側付記の計測値への参照は `agent-docs/web/gates/p5-01-latency.md` に直した。
- 参照の付け替え: `docs/frontend/DESIGN.md`・`docs/frontend/FRONTEND_CONTRACT.md` の ADR-0081 と web-0004 への相対リンクを `../../agent-docs/...` に直した。`web/` に web-0004 への参照は無かった。
- `docs/frontend/{DESIGN,FRONTEND_CONTRACT,UX_AUDIT}.md` は今の path のまま。`check-doc-layout.sh` は分類を求めなかったので `docs-layout.tsv` は変えていない。
- `web/` に衝突は無く、`crates/`・`gui/` は main と同じ。

## 検査

- `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `progress-index.sh --check` / `check-doc-layout.sh scripts/dev/docs-layout.tsv`: いずれも exit 0。

## 未解決

- 生成型（`web/api/generated`）と web の静的・単体検査・build は後続の web-verify で確かめる。

## web 検査（2026-10-04、WU web-verify）

merge 後の木（`f24bbed8`）で実行。web/ の修正は不要だった。

- `corepack pnpm@12.6.0 -C web install --frozen-lockfile` → exit 0
- `corepack pnpm@12.6.0 -C web gen:types` → exit 0、`web/api/generated` の差分ゼロ（main の `docs/api/v1/api-v1.schema.json` と一致）
- `typecheck` exit 0 / `lint` exit 0（biome: 既存の warning 4・info 1）/ `test` exit 0（42 files・269 tests passed）/ `build` exit 0
- `check:boundaries`・`check:parity`・`check:secrets` → いずれも exit 0
- doc 検査 4 本（check-doc-links・check-adr-numbers・progress-index --check・check-doc-layout）→ exit 0
- `git diff --quiet c1b24fb6fa8c -- crates/ gui/` → exit 0（main と同じ）
