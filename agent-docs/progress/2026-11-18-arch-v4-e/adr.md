---
title: アーキ v4 E adr — 2 領域の対象 file・狭い操作・数え方・証明の方法の設計と before 値
tasks: [01M7MZTKB6GQ241F6JJYSPJNS1]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# adr（WorkUnit）

完了日: 2026-11-18

## 変更
- `arch-v4 E adr: …`（件名で参照。hash は書かない）— [ADR 2026-11-18 dispatcher-store-access-pilot](../../adr/2026-11-18-dispatcher-store-access-pilot.md) を足した
  （節: 対象 file と狭い操作・数え方と数えないもの・証明・反例試験・撤去対象・実装前 brief）。
- [進捗](../2026-11-18-arch-v4-e.md) に before 値を書いた。[measure-commands.md](measure-commands.md) に A の C1・C2・C3・C7b・C9・C12 を写し、E1〜E3 を足した。
- `crates/`・`scripts/`・`web/` は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- C3（measure-commands.md）を main bc665168 と HEAD に当てた → 69、44/633、35/218、論理 76、48/661、39/224（両方同じ）。
- E1・E2・E3 → 進捗の before 値の表。
- 反例試験 12 本の存在: `git grep -l "fn <名>\b" -- crates` → 全部見つかった（cex_lease_restart.rs・cex_review_authority.rs・cex_late_completion.rs）。
- `git diff --name-only $CELERIS_WU_BASE -- crates scripts web` → 空。
- `sh scripts/dev/progress-index.sh --check` → 結果は下の「検査」に書く。

## 検査
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `git diff --name-only $CELERIS_WU_BASE -- crates scripts web | wc -l` → 0。
- `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`、repo-health は doc-only）。初回は exit 1 だった:
  main に無い D の ADR への link が壊れていた（repo-health の new 1 件）。link を平文に直して再実行した。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` は実行していない（Rust の差分 0。run の壁時計を優先した）。
